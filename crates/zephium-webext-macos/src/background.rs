//! WebKit can restore a service-worker registration after an unclean exit
//! without starting its worker, while reporting successful background loading.
//! Verify the prepared worker's startup signal before opening a popup. Repair
//! only that worker's registration from its own native background page.

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::{Rc, Weak};

use block2::RcBlock;
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::AnyObject;
use objc2::{msg_send, sel, Message};
use objc2_foundation::{
    NSError, NSObjectProtocol, NSRunLoop, NSRunLoopCommonModes, NSString, NSTimer,
};
use objc2_web_kit::{WKContentWorld, WKWebExtensionContext, WKWebView};

use crate::runtime::Shared;

type Done = Box<dyn FnOnce(Result<(), String>)>;
const START_SECONDS: f64 = 5.0;
const MAX_WAITERS: usize = 64;

pub(crate) struct State {
    verify: bool,
    ready: RefCell<ObjcWeak<WKWebView>>,
    pending: RefCell<Option<Rc<Attempt>>>,
}

impl State {
    pub(crate) fn new(verify: bool) -> Self {
        Self {
            verify,
            ready: RefCell::default(),
            pending: RefCell::default(),
        }
    }

    pub(crate) fn cancel(&self) {
        let pending = self.pending.borrow_mut().take();
        if let Some(pending) = pending {
            pending.finish(Err("The extension was unloaded.".into()));
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        if let Some(pending) = self.pending.get_mut().take() {
            pending.finish(Err("The extension runtime was closed.".into()));
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Repairing,
    Settled,
}

struct Attempt {
    state: Weak<State>,
    shared: Weak<Shared>,
    context: Retained<WKWebExtensionContext>,
    view: Retained<WKWebView>,
    phase: Cell<Phase>,
    waiters: RefCell<Vec<Done>>,
    timer: RefCell<Option<Retained<NSTimer>>>,
}

fn state_for(shared: &Shared, context: &WKWebExtensionContext) -> Option<Rc<State>> {
    shared
        .loaded
        .borrow()
        .values()
        .find(|loaded| std::ptr::eq(&*loaded.context, context))
        .map(|loaded| loaded.background.clone())
}

fn view(context: &WKWebExtensionContext) -> Option<Retained<WKWebView>> {
    if !context.respondsToSelector(sel!(_backgroundWebView)) {
        return None;
    }
    // SAFETY: guarded, read-only WebKit getter, called on its main thread.
    unsafe { msg_send![context, _backgroundWebView] }
}

fn worker_script(context: &WKWebExtensionContext) -> Option<(String, bool)> {
    // SAFETY: the loaded context and its manifest are main-thread WebKit objects.
    let manifest = unsafe { context.webExtension().manifest() };
    let manifest = crate::json::from_object(Some(&manifest));
    let background = manifest.get("background")?;
    let path = background.get("service_worker")?.as_str()?;
    let base = unsafe { context.baseURL() }.absoluteString()?.to_string();
    let base = url::Url::parse(&base).ok()?;
    let script = base.join(path).ok()?;
    if !same_origin(&base, &script) {
        return None;
    }
    Some((
        script.into(),
        background.get("type").and_then(|v| v.as_str()) == Some("module"),
    ))
}

fn same_origin(base: &url::Url, target: &url::Url) -> bool {
    base.scheme() == target.scheme()
        && base.host_str() == target.host_str()
        && base.port() == target.port()
}

pub(crate) fn wait(shared: &Rc<Shared>, context: &WKWebExtensionContext, done: Done) {
    let Some(state) = state_for(shared, context) else {
        return done(Err("The extension was unloaded.".into()));
    };
    // Background pages and unadapted lab fixtures do not send our worker signal.
    if !state.verify || worker_script(context).is_none() {
        return done(Ok(()));
    }
    let Some(view) = view(context) else {
        return done(Err("The extension background view is unavailable.".into()));
    };
    let previous = state.pending.borrow().clone();
    if let Some(previous) = previous.filter(|previous| !std::ptr::eq(&*previous.view, &*view)) {
        previous.finish(Err("The extension background was replaced.".into()));
    }
    // Settling an older attempt invokes callers, which may unload this context
    // or start an attempt for this view.
    if !state_for(shared, context).is_some_and(|current| Rc::ptr_eq(&current, &state)) {
        return done(Err("The extension was unloaded.".into()));
    }
    let pending = state.pending.borrow().clone();
    if let Some(pending) = pending.filter(|pending| std::ptr::eq(&*pending.view, &*view)) {
        if pending.waiters.borrow().len() >= MAX_WAITERS {
            return done(Err("Too many extension background requests.".into()));
        }
        pending.waiters.borrow_mut().push(done);
        return;
    }
    let ready = state
        .ready
        .borrow()
        .load()
        .is_some_and(|ready| std::ptr::eq(&*ready, &*view));
    if ready {
        return done(Ok(()));
    }
    let attempt = Rc::new(Attempt {
        state: Rc::downgrade(&state),
        shared: Rc::downgrade(shared),
        context: context.retain(),
        view,
        phase: Cell::new(Phase::Waiting),
        waiters: RefCell::new(vec![done]),
        timer: RefCell::default(),
    });
    *state.pending.borrow_mut() = Some(attempt.clone());
    attempt.arm();
}

pub(crate) fn started(shared: &Rc<Shared>, context: &WKWebExtensionContext) {
    let (Some(state), Some(view)) = (state_for(shared, context), view(context)) else {
        return;
    };
    *state.ready.borrow_mut() = ObjcWeak::from_retained(&view);
    let pending = state.pending.borrow().clone();
    if let Some(pending) = pending {
        // A repair also waits for the new registration to activate. A late
        // startup from the old worker cannot release its waiters early.
        if pending.phase.get() == Phase::Waiting && std::ptr::eq(&*pending.view, &*view) {
            pending.finish(Ok(()));
        }
    }
}

impl Attempt {
    fn current(&self) -> bool {
        self.shared
            .upgrade()
            .is_some_and(|shared| state_for(&shared, &self.context).is_some())
            && view(&self.context).is_some_and(|view| std::ptr::eq(&*view, &*self.view))
    }

    fn finish(&self, result: Result<(), String>) {
        if self.phase.replace(Phase::Settled) == Phase::Settled {
            return;
        }
        let result = if result.is_ok() && !self.current() {
            Err("The extension background was replaced.".into())
        } else {
            result
        };
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
        if let Some(state) = self.state.upgrade() {
            *state.ready.borrow_mut() = if result.is_ok() {
                ObjcWeak::from_retained(&self.view)
            } else {
                ObjcWeak::default()
            };
            let ours = state
                .pending
                .borrow()
                .as_ref()
                .is_some_and(|pending| std::ptr::eq(&**pending, self));
            if ours {
                state.pending.borrow_mut().take();
            }
        }
        let waiters = std::mem::take(&mut *self.waiters.borrow_mut());
        for done in waiters {
            done(result.clone());
        }
    }

    fn arm(self: &Rc<Self>) {
        if let Some(timer) = self.timer.borrow_mut().take() {
            timer.invalidate();
        }
        let weak = Rc::downgrade(self);
        let fire = RcBlock::new(move |_: NonNull<NSTimer>| {
            let Some(attempt) = weak.upgrade() else {
                return;
            };
            if !attempt.current() {
                attempt.finish(Err("The extension background was replaced.".into()));
            } else if attempt.phase.get() == Phase::Waiting {
                attempt.repair();
            } else if attempt.phase.get() == Phase::Repairing {
                attempt.finish(Err("The extension background did not start.".into()));
            }
        });
        // SAFETY: native main thread; NSTimer copies the block. Only a weak
        // reference is captured, and completion invalidates the timer.
        let timer =
            unsafe { NSTimer::timerWithTimeInterval_repeats_block(START_SECONDS, false, &fire) };
        // SAFETY: the main run loop and mode constant are valid here. Menu
        // tracking must not suspend a background-start deadline.
        unsafe {
            NSRunLoop::mainRunLoop().addTimer_forMode(&timer, NSRunLoopCommonModes);
        }
        *self.timer.borrow_mut() = Some(timer);
    }

    fn repair(self: &Rc<Self>) {
        let Some((expected, modules)) = worker_script(&self.context) else {
            return self.finish(Err("The extension worker URL is invalid.".into()));
        };
        let Some(shared) = self.shared.upgrade() else {
            return;
        };
        self.phase.set(Phase::Repairing);
        self.arm();
        let script = format!(
            "const expected = {}; const modules = {};\n{}",
            serde_json::to_string(&expected).expect("a URL string is JSON"),
            modules,
            include_str!("background/recover.js")
        );
        let weak = Rc::downgrade(self);
        let complete = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
            let Some(attempt) = weak.upgrade() else {
                return;
            };
            let ok = error.is_null()
                && crate::json::from_object(unsafe { value.as_ref() }).as_bool() == Some(true);
            attempt.finish(if ok {
                Ok(())
            } else {
                Err("The extension worker could not be recovered.".into())
            });
        });
        // SAFETY: evaluate only in this context's native background page.
        // Unloading the context closes that view; callbacks also check identity.
        unsafe {
            self.view
                .callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                    &NSString::from_str(&script),
                    None,
                    None,
                    &WKContentWorld::pageWorld(shared.mtm),
                    Some(&complete),
                );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::same_origin;

    #[test]
    fn worker_recovery_stays_in_its_extension_origin() {
        let base = url::Url::parse("chrome-extension://example/").unwrap();
        assert!(same_origin(
            &base,
            &base.join("background/worker.js").unwrap()
        ));
        for other in [
            "https://example/worker.js",
            "chrome-extension://other/worker.js",
            "chrome-extension://example:81/worker.js",
        ] {
            assert!(!same_origin(&base, &url::Url::parse(other).unwrap()));
        }
    }
}
