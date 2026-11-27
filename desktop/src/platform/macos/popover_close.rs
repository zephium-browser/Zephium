//! An extension's popup hangs from a hidden sidebar's card, which stays out
//! while the popup is open. A click on the page closes the popup but leaves
//! the keyboard with the page, so the chrome never learns it went; the
//! closing popover tells it instead.

use std::cell::RefCell;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::NSPopoverDidCloseNotification;
use objc2_foundation::{MainThreadMarker, NSNotification, NSNotificationCenter};
use tauri::{Manager, WebviewWindow};

thread_local! {
    static WINDOWS: RefCell<Vec<WebviewWindow>> = const { RefCell::new(Vec::new()) };
    static OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(None) };
}

/// Tells `window`'s chrome when a popover closes while its sidebar is hidden.
/// Runs on the main thread.
pub fn watch(window: &WebviewWindow, on: bool) {
    if MainThreadMarker::new().is_none() {
        return;
    }
    let any = WINDOWS.with(|windows| {
        let mut windows = windows.borrow_mut();
        windows.retain(|watched| watched.label() != window.label());
        if on {
            windows.push(window.clone());
        }
        !windows.is_empty()
    });
    OBSERVER.with(|observer| {
        let mut observer = observer.borrow_mut();
        let center = NSNotificationCenter::defaultCenter();
        match (any, observer.as_ref()) {
            (true, None) => {
                let closed = RcBlock::new(|_: NonNull<NSNotification>| {
                    let windows = WINDOWS.with(|windows| windows.borrow().clone());
                    for window in windows {
                        crate::emit_to_privileged(
                            window.app_handle(),
                            window.label(),
                            "zephium:popover-closed",
                            &serde_json::Value::Null,
                        );
                    }
                });
                *observer = Some(unsafe {
                    center.addObserverForName_object_queue_usingBlock(
                        Some(NSPopoverDidCloseNotification),
                        None,
                        None,
                        &closed,
                    )
                });
            }
            (false, Some(_)) => {
                if let Some(token) = observer.take() {
                    unsafe { center.removeObserver(token.as_ref()) };
                }
            }
            _ => {}
        }
    });
}
