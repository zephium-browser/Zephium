use std::cell::Cell;
use std::ptr::NonNull;
use std::rc::{Rc, Weak as RcWeak};

use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, ProtocolObject};
use objc2::{define_class, msg_send, DefinedClass, MainThreadMarker, MainThreadOnly, Message};
use objc2_foundation::{NSArray, NSDate, NSError, NSObjectProtocol, NSSet, NSString, NSURL};
use objc2_web_kit::{
    WKWebExtensionAction, WKWebExtensionContext, WKWebExtensionController,
    WKWebExtensionControllerDelegate, WKWebExtensionMatchPattern, WKWebExtensionMessagePort,
    WKWebExtensionPermission, WKWebExtensionTab, WKWebExtensionTabConfiguration,
    WKWebExtensionWindow, WKWebExtensionWindowConfiguration,
};

use crate::runtime::Shared;
use crate::{access, bridge, error, native, socket, TabRequest};

pub(crate) struct Ivars {
    shared: RcWeak<Shared>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "ZephiumWebExtControllerDelegate"]
    #[ivars = Ivars]
    pub(crate) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl WKWebExtensionControllerDelegate for Delegate {
        #[unsafe(method_id(webExtensionController:openWindowsForExtensionContext:))]
        fn open_windows(
            &self,
            _controller: &WKWebExtensionController,
            _context: &WKWebExtensionContext,
        ) -> Retained<NSArray<ProtocolObject<dyn WKWebExtensionWindow>>> {
            let windows: Vec<_> = self
                .shared()
                .map(|shared| shared.graph.borrow().windows.clone())
                .unwrap_or_default()
                .into_iter()
                .map(ProtocolObject::from_retained)
                .collect();
            NSArray::from_retained_slice(&windows)
        }

        #[unsafe(method_id(webExtensionController:focusedWindowForExtensionContext:))]
        fn focused_window(
            &self,
            _controller: &WKWebExtensionController,
            _context: &WKWebExtensionContext,
        ) -> Option<Retained<ProtocolObject<dyn WKWebExtensionWindow>>> {
            let window = self
                .shared()
                .and_then(|shared| shared.graph.borrow().focused_window());
            window.map(ProtocolObject::from_retained)
        }

        #[unsafe(method(webExtensionController:openNewTabUsingConfiguration:forExtensionContext:completionHandler:))]
        fn open_new_tab(
            &self,
            _controller: &WKWebExtensionController,
            configuration: &WKWebExtensionTabConfiguration,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut ProtocolObject<dyn WKWebExtensionTab>, *mut NSError)>,
        ) {
            let completion = completion.copy();
            let Some(shared) = self.shared() else {
                return completion.call((
                    std::ptr::null_mut(),
                    Retained::as_ptr(&error("The browser is closing.")).cast_mut(),
                ));
            };
            let request = unsafe {
                TabRequest::Create {
                    window: configuration.window().and_then(|window| {
                        let window: &AnyObject = window.as_ref();
                        window_id(&shared, window)
                    }),
                    url: configuration
                        .url()
                        .and_then(|url| url.absoluteString())
                        .map(|url| url.to_string()),
                    active: configuration.shouldBeActive(),
                    pinned: configuration.shouldBePinned(),
                    index: Some(configuration.index()).filter(|index| *index != usize::MAX),
                }
            };
            let weak = Rc::downgrade(&shared);
            shared.host().tab_request(
                request,
                Box::new(move |result| {
                    let tab = match (result, weak.upgrade()) {
                        (Ok(Some(id)), Some(shared)) => shared.graph.borrow().tab(id),
                        (Err(message), _) => {
                            let error = error(&message);
                            return completion
                                .call((std::ptr::null_mut(), Retained::as_ptr(&error).cast_mut()));
                        }
                        _ => None,
                    };
                    match tab {
                        Some(tab) => {
                            let tab = ProtocolObject::<dyn WKWebExtensionTab>::from_retained(tab);
                            completion
                                .call((Retained::as_ptr(&tab).cast_mut(), std::ptr::null_mut()));
                        }
                        None => completion.call((std::ptr::null_mut(), std::ptr::null_mut())),
                    }
                }),
            );
        }

        // Extension windows open as tabs of the focused window.
        #[unsafe(method(webExtensionController:openNewWindowUsingConfiguration:forExtensionContext:completionHandler:))]
        fn open_new_window(
            &self,
            _controller: &WKWebExtensionController,
            configuration: &WKWebExtensionWindowConfiguration,
            context: &WKWebExtensionContext,
            completion: &DynBlock<
                dyn Fn(*mut ProtocolObject<dyn WKWebExtensionWindow>, *mut NSError),
            >,
        ) {
            let completion = completion.copy();
            let Some(shared) = self.shared() else {
                return completion.call((std::ptr::null_mut(), std::ptr::null_mut()));
            };
            let urls: Vec<String> = unsafe { configuration.tabURLs() }
                .iter()
                .filter_map(|url| url.absoluteString().map(|url| url.to_string()))
                .collect();
            // SAFETY: WebKit passes `context` as a valid extension context for
            // the duration of this delegate callback.
            let extension = unsafe { context.uniqueIdentifier() }.to_string();
            // Answered once every tab is open and recorded, so a windows.remove
            // right after windows.create finds them.
            let answer = Rc::new(move |shared: Option<Rc<Shared>>| {
                let window = shared.and_then(|shared| shared.graph.borrow().focused_window());
                match window {
                    Some(window) => {
                        let window =
                            ProtocolObject::<dyn WKWebExtensionWindow>::from_retained(window);
                        completion
                            .call((Retained::as_ptr(&window).cast_mut(), std::ptr::null_mut()));
                    }
                    None => completion.call((std::ptr::null_mut(), std::ptr::null_mut())),
                }
            });
            if urls.is_empty() {
                return answer(Some(shared));
            }
            let pending = Rc::new(Cell::new(urls.len()));
            for url in &urls {
                // Remembered so the extension's windows.remove closes these.
                let weak = Rc::downgrade(&shared);
                let extension = extension.clone();
                let pending = pending.clone();
                let answer = answer.clone();
                shared.host().tab_request(
                    TabRequest::Create {
                        window: None,
                        url: Some(url.clone()),
                        active: true,
                        pinned: false,
                        index: None,
                    },
                    Box::new(move |result| {
                        let shared = weak.upgrade();
                        if let (Ok(Some(tab)), Some(shared)) = (result, &shared) {
                            shared.opened.record(&extension, tab);
                        }
                        pending.set(pending.get() - 1);
                        if pending.get() == 0 {
                            answer(shared);
                        }
                    }),
                );
            }
        }

        #[unsafe(method(webExtensionController:openOptionsPageForExtensionContext:completionHandler:))]
        fn open_options(
            &self,
            _controller: &WKWebExtensionController,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let id = unsafe { context.uniqueIdentifier() }.to_string();
            match (
                self.shared(),
                unsafe { context.optionsPageURL() }.and_then(|url| url.absoluteString()),
            ) {
                (Some(shared), Some(url)) => {
                    shared.host().open_options(&id, &url.to_string());
                    completion.call((std::ptr::null_mut(),));
                }
                _ => {
                    let error = error("This extension has no options page.");
                    completion.call((Retained::as_ptr(&error).cast_mut(),));
                }
            }
        }

        #[unsafe(method(webExtensionController:promptForPermissions:inTab:forExtensionContext:completionHandler:))]
        fn prompt_permissions(
            &self,
            _controller: &WKWebExtensionController,
            permissions: &NSSet<WKWebExtensionPermission>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(NonNull<NSSet<WKWebExtensionPermission>>, *mut NSDate)>,
        ) {
            let requested: Vec<String> = permissions.iter().map(|p| p.to_string()).collect();
            let Some(shared) = self.shared() else {
                let none = NSSet::<WKWebExtensionPermission>::new();
                return completion.call((NonNull::from(&*none), std::ptr::null_mut()));
            };
            let granted = permissions.retain();
            let completion = completion.copy();
            let extension = unsafe { context.uniqueIdentifier() }.to_string();
            access::request(
                &shared,
                &extension,
                requested,
                Vec::new(),
                Box::new(move |allowed| {
                    let none = NSSet::<WKWebExtensionPermission>::new();
                    let answer = if allowed { &granted } else { &none };
                    completion.call((NonNull::from(&**answer), std::ptr::null_mut()));
                }),
            );
        }

        // Chrome never prompts when an extension touches a page it lacks
        // access to; the call simply fails. WebKit asks here instead.
        #[unsafe(method(webExtensionController:promptForPermissionToAccessURLs:inTab:forExtensionContext:completionHandler:))]
        fn prompt_urls(
            &self,
            _controller: &WKWebExtensionController,
            _urls: &NSSet<NSURL>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            _context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(NonNull<NSSet<NSURL>>, *mut NSDate)>,
        ) {
            let none = NSSet::<NSURL>::new();
            completion.call((NonNull::from(&*none), std::ptr::null_mut()));
        }

        #[unsafe(method(webExtensionController:promptForPermissionMatchPatterns:inTab:forExtensionContext:completionHandler:))]
        fn prompt_patterns(
            &self,
            _controller: &WKWebExtensionController,
            patterns: &NSSet<WKWebExtensionMatchPattern>,
            _tab: Option<&ProtocolObject<dyn WKWebExtensionTab>>,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(NonNull<NSSet<WKWebExtensionMatchPattern>>, *mut NSDate)>,
        ) {
            let requested: Vec<String> = patterns
                .iter()
                .map(|p| unsafe { p.string() }.to_string())
                .collect();
            let Some(shared) = self.shared() else {
                let none = NSSet::<WKWebExtensionMatchPattern>::new();
                return completion.call((NonNull::from(&*none), std::ptr::null_mut()));
            };
            let granted = patterns.retain();
            let completion = completion.copy();
            let extension = unsafe { context.uniqueIdentifier() }.to_string();
            access::request(
                &shared,
                &extension,
                Vec::new(),
                requested,
                Box::new(move |allowed| {
                    let none = NSSet::<WKWebExtensionMatchPattern>::new();
                    let answer = if allowed { &granted } else { &none };
                    completion.call((NonNull::from(&**answer), std::ptr::null_mut()));
                }),
            );
        }

        #[unsafe(method(webExtensionController:presentPopupForAction:forExtensionContext:completionHandler:))]
        fn present_popup(
            &self,
            _controller: &WKWebExtensionController,
            action: &WKWebExtensionAction,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let id = unsafe { context.uniqueIdentifier() }.to_string();
            let shown = self
                .shared()
                .is_some_and(|shared| shared.host().present_popup(&id, action));
            if !shown {
                unsafe { action.closePopup() };
                let error = error("The popup could not be shown.");
                return completion.call((Retained::as_ptr(&error).cast_mut(),));
            }
            completion.call((std::ptr::null_mut(),));
        }

        #[unsafe(method(webExtensionController:sendMessage:toApplicationWithIdentifier:forExtensionContext:replyHandler:))]
        fn send_message(
            &self,
            _controller: &WKWebExtensionController,
            message: &AnyObject,
            application: Option<&NSString>,
            context: &WKWebExtensionContext,
            reply: &DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>,
        ) {
            let application = application.map(|id| id.to_string()).unwrap_or_default();
            match self.shared() {
                Some(shared) if application == bridge::APPLICATION => {
                    bridge::handle(&shared, context, message, reply);
                }
                Some(shared) => {
                    if let Err(message) =
                        native::send(&shared, context, &application, message, reply)
                    {
                        let error = error(&message);
                        reply.call((std::ptr::null_mut(), Retained::as_ptr(&error).cast_mut()));
                    }
                }
                None => {
                    let error = error("Specified native messaging host not found.");
                    reply.call((std::ptr::null_mut(), Retained::as_ptr(&error).cast_mut()));
                }
            }
        }

        #[unsafe(method(webExtensionController:connectUsingMessagePort:forExtensionContext:completionHandler:))]
        fn connect(
            &self,
            _controller: &WKWebExtensionController,
            port: &WKWebExtensionMessagePort,
            context: &WKWebExtensionContext,
            completion: &DynBlock<dyn Fn(*mut NSError)>,
        ) {
            let application = unsafe { port.applicationIdentifier() }
                .map(|id| id.to_string())
                .unwrap_or_default();
            match self.shared() {
                Some(shared) if application == socket::APPLICATION => {
                    socket::connect(&shared, context, port);
                    completion.call((std::ptr::null_mut(),));
                }
                Some(shared) => match native::connect(&shared, context, port, &application) {
                    Ok(()) => completion.call((std::ptr::null_mut(),)),
                    Err(message) => {
                        let error = error(&message);
                        completion.call((Retained::as_ptr(&error).cast_mut(),));
                    }
                },
                None => {
                    let error = error("Specified native messaging host not found.");
                    completion.call((Retained::as_ptr(&error).cast_mut(),));
                }
            }
        }
    }
);

impl Delegate {
    pub(crate) fn new(mtm: MainThreadMarker, shared: RcWeak<Shared>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars { shared });
        unsafe { msg_send![super(this), init] }
    }

    fn shared(&self) -> Option<Rc<Shared>> {
        self.ivars().shared.upgrade()
    }
}

fn window_id(shared: &Shared, window: &AnyObject) -> Option<u64> {
    let graph = shared.graph.borrow();
    graph
        .windows
        .iter()
        .find(|candidate| std::ptr::eq(Retained::as_ptr(candidate).cast::<AnyObject>(), window))
        .map(|window| window.id())
}
