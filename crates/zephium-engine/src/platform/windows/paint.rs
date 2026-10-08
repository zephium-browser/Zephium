//! A bounded visual handoff, independent of navigation/security admission;
//! the Windows half of macos/paint.rs. WebView2 paints its white default
//! until the new document's first contentful paint, so the stage keeps an
//! opaque cover in the page ground over it until then.
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use webview2_com::ExecuteScriptCompletedHandler;
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2;
use wry::WebViewExtWindows;
use zephium_core::ids::ItemId;

use super::{schedule_browser_timeout, ContentPolicyTimeout, Stage};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
const MAX_COVER_TIME: Duration = Duration::from_millis(600);
// ExecuteScript does not await promises and WebView2 has no paint event, so
// the handoff asks again each display frame or two.
const POLL_INTERVAL: Duration = Duration::from_millis(32);
// One more poll after the answer lets the painted frame reach the screen.
const SETTLE_POLLS: u8 = 1;

struct State {
    id: ItemId,
    token: u64,
    stages: Vec<Stage>,
    core: ICoreWebView2,
    finished: Cell<bool>,
    painted: Cell<Option<u8>>,
    timeout: RefCell<Option<ContentPolicyTimeout>>,
    poll: RefCell<Option<ContentPolicyTimeout>>,
}

impl State {
    fn finish(&self) {
        if self.finished.replace(true) {
            return;
        }
        self.timeout.borrow_mut().take();
        self.poll.borrow_mut().take();
        for stage in &self.stages {
            stage.uncover(self.id, self.token);
        }
    }
}

/// Lives with the exact native view. Closing/replacing it cancels the handoff.
pub(crate) struct PaintCover(Rc<State>);

impl PaintCover {
    pub(crate) fn begin(
        id: ItemId,
        view: &wry::WebView,
        stages: impl Iterator<Item = Stage>,
    ) -> Option<Self> {
        let token = NEXT_TOKEN
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
            .ok()?;
        let core = view.webview();
        let stages: Vec<_> = stages.filter(|stage| stage.cover(id, token)).collect();
        if stages.is_empty() {
            return None;
        }
        let state = Rc::new(State {
            id,
            token,
            stages,
            core,
            finished: Cell::new(false),
            painted: Cell::new(None),
            timeout: RefCell::new(None),
            poll: RefCell::new(None),
        });
        let expiring = Rc::downgrade(&state);
        let Some(timeout) = schedule_browser_timeout(MAX_COVER_TIME, move || {
            if let Some(state) = expiring.upgrade() {
                state.finish();
            }
        }) else {
            state.finish();
            return None;
        };
        *state.timeout.borrow_mut() = Some(timeout);
        ask(&state);
        Some(Self(state))
    }
}

impl Drop for PaintCover {
    fn drop(&mut self) {
        self.0.finish();
    }
}

/// Asks the committed document whether it has painted content. The answer is
/// only a hint to remove an inert cover: a page that delays or forges it moves
/// the bounded cover's removal and grants nothing else.
fn ask(state: &Rc<State>) {
    if state.finished.get() {
        return;
    }
    if let Some(left) = state.painted.get() {
        if left == 0 {
            state.finish();
        } else {
            state.painted.set(Some(left - 1));
            schedule_poll(state);
        }
        return;
    }
    let answered = Rc::downgrade(state);
    let callback = ExecuteScriptCompletedHandler::create(Box::new(move |result, value| {
        if let Some(state) = answered.upgrade() {
            if result.is_ok() && value == "true" {
                state.painted.set(Some(SETTLE_POLLS));
            }
            schedule_poll(&state);
        }
        Ok(())
    }));
    // SAFETY: a fixed expression on the live controller's STA; it reads one
    // performance entry and touches no DOM.
    let sent = unsafe {
        state.core.ExecuteScript(
            windows_core::w!("performance.getEntriesByName('first-contentful-paint').length > 0"),
            &callback,
        )
    };
    if sent.is_err() {
        state.finish();
    }
}

fn schedule_poll(state: &Rc<State>) {
    if state.finished.get() {
        return;
    }
    let next: Weak<State> = Rc::downgrade(state);
    let poll = schedule_browser_timeout(POLL_INTERVAL, move || {
        if let Some(state) = next.upgrade() {
            ask(&state);
        }
    });
    match poll {
        Some(poll) => *state.poll.borrow_mut() = Some(poll),
        None => state.finish(),
    }
}
