//! Owned GTK dialogs for ordinary pages; answers never outlive their document.
use super::permits::EventPermit;
use crate::navigation_epoch::{NavigationActivity, NavigationEpochTracker};
use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use webkit2gtk::{ScriptDialog, ScriptDialogType, WebView};

thread_local! { static BUSY: Cell<bool> = const { Cell::new(false) }; }
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        BUSY.with(|busy| busy.set(false));
    }
}

struct Pending {
    identity: Rc<()>,
    activity: NavigationActivity,
    before_unload: bool,
    page: gtk::glib::WeakRef<WebView>,
    owner: gtk::Window,
    panel: gtk::Dialog,
    entry: Option<gtk::Entry>,
    suppress: gtk::CheckButton,
    native: Option<ScriptDialog>,
    _lease: Lease,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.panel.close();
        if let Some(native) = self.native.take() {
            native.close();
        }
    }
}

struct Handoff {
    activity: NavigationActivity,
    page: gtk::glib::WeakRef<WebView>,
    owner: gtk::Window,
    native: Option<ScriptDialog>,
    focus: Option<gtk::glib::SignalHandlerId>,
    deadline: Option<gtk::glib::SourceId>,
}
impl Drop for Handoff {
    fn drop(&mut self) {
        if let Some(signal) = self.focus.take() {
            self.owner.disconnect(signal);
        }
        if let Some(deadline) = self.deadline.take() {
            deadline.remove();
        }
        if let Some(native) = self.native.take() {
            gtk::glib::idle_add_local_once(move || native.close());
        }
    }
}

pub(super) struct ScriptDialogs {
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    presentation: Arc<AtomicBool>,
    pending: RefCell<Option<Pending>>,
    handoff: RefCell<Option<Handoff>>,
    last: Cell<Option<Instant>>,
    silenced: Cell<bool>,
    cancelled_load: Box<dyn Fn(Option<zephium_core::ports::engine::NavigationRequestId>)>,
}
impl ScriptDialogs {
    pub(super) fn new(
        permit: EventPermit,
        navigation: NavigationEpochTracker,
        presentation: Arc<AtomicBool>,
        cancelled_load: impl Fn(Option<zephium_core::ports::engine::NavigationRequestId>) + 'static,
    ) -> Rc<Self> {
        Rc::new(Self {
            permit,
            navigation,
            presentation,
            pending: RefCell::new(None),
            handoff: RefCell::new(None),
            last: Cell::new(None),
            silenced: Cell::new(false),
            cancelled_load: Box::new(cancelled_load),
        })
    }
    pub(super) fn navigation_started(&self) {
        self.cancel();
        self.silenced.set(false);
        self.last.set(None);
    }
    pub(super) fn cancel(&self) {
        if let Some(handoff) = self.handoff.borrow_mut().take() {
            self.settle_cancelled_load(handoff.activity);
            drop(handoff);
        }
        let pending = self.pending.borrow_mut().take();
        drop(pending);
    }
    fn current(&self, activity: NavigationActivity, before_unload: bool) -> bool {
        self.permit.active_token().is_some()
            && self.navigation.matches_activity(activity)
            && if before_unload {
                self.navigation.resident_document_snapshot().is_some()
            } else {
                self.navigation.current_committed().is_some()
            }
            && self.presentation.load(Ordering::Acquire)
    }
    pub(super) fn present(self: &Rc<Self>, page: &WebView, native: &ScriptDialog) -> bool {
        let activity = self.navigation.activity_snapshot();
        let handled = self.present_owned(page, native) || self.await_foreground(page, native);
        if !handled && native.dialog_type() == ScriptDialogType::BeforeUnloadConfirm {
            if let Some(activity) = activity {
                self.settle_cancelled_load(activity);
            }
        }
        handled
    }
    fn await_foreground(self: &Rc<Self>, page: &WebView, native: &ScriptDialog) -> bool {
        if native.dialog_type() != ScriptDialogType::BeforeUnloadConfirm
            || self.pending.borrow().is_some()
            || self.handoff.borrow().is_some()
        {
            return false;
        }
        let Some(activity) = self.navigation.activity_snapshot() else {
            return false;
        };
        let Some(owner) = page
            .toplevel()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok())
        else {
            return false;
        };
        if !self.current(activity, true)
            || !self.navigation.awaiting_browser_navigation(activity)
            || !page.is_mapped()
            || !owner.is_visible()
            || owner.is_active()
        {
            return false;
        }
        let weak = Rc::downgrade(self);
        let focus = owner.connect_is_active_notify(move |owner| {
            if owner.is_active() {
                if let Some(broker) = weak.upgrade() {
                    broker.finish_handoff(false);
                }
            }
        });
        let weak = Rc::downgrade(self);
        let deadline = gtk::glib::timeout_add_local_once(Duration::from_secs(2), move || {
            if let Some(broker) = weak.upgrade() {
                broker.finish_handoff(true);
            }
        });
        *self.handoff.borrow_mut() = Some(Handoff {
            activity,
            page: page.downgrade(),
            owner,
            native: Some(native.clone()),
            focus: Some(focus),
            deadline: Some(deadline),
        });
        true
    }
    fn finish_handoff(self: &Rc<Self>, expired: bool) {
        let Some(mut handoff) = self.handoff.borrow_mut().take() else {
            return;
        };
        if expired {
            handoff.deadline.take();
        }
        let valid = !expired
            && self.current(handoff.activity, true)
            && self
                .navigation
                .awaiting_browser_navigation(handoff.activity);
        if valid {
            if let (Some(page), Some(native)) = (handoff.page.upgrade(), handoff.native.as_ref()) {
                if self.present_owned(&page, native) {
                    handoff.native.take();
                    return;
                }
            }
        }
        self.settle_cancelled_load(handoff.activity);
    }

    fn settle_cancelled_load(&self, activity: NavigationActivity) {
        if self.permit.active_token().is_some() {
            if let Some(request) = self.navigation.cancel_beforeunload(activity) {
                (self.cancelled_load)(request);
            }
        }
    }
    fn present_owned(self: &Rc<Self>, page: &WebView, native: &ScriptDialog) -> bool {
        let Some(activity) = self.navigation.activity_snapshot() else {
            return false;
        };
        let Some(owner) = page
            .toplevel()
            .and_then(|widget| widget.downcast::<gtk::Window>().ok())
        else {
            return false;
        };
        let kind = native.dialog_type();
        let before_unload = kind == ScriptDialogType::BeforeUnloadConfirm;
        if self.silenced.get()
            || !self.current(activity, before_unload)
            || !page.is_mapped()
            || !owner.is_active()
            || self.pending.borrow().is_some()
        {
            return false;
        }
        if !matches!(
            kind,
            ScriptDialogType::Alert
                | ScriptDialogType::Confirm
                | ScriptDialogType::Prompt
                | ScriptDialogType::BeforeUnloadConfirm
        ) {
            return false;
        }
        // SAFETY: WebKit owns these NUL-terminated strings for the retained dialog.
        let Some(message) = bounded_native_text(unsafe {
            webkit2gtk::ffi::webkit_script_dialog_get_message(native.to_glib_none().0)
        }) else {
            return false;
        };
        let default = if kind == ScriptDialogType::Prompt {
            // SAFETY: this accessor is valid only for the checked Prompt kind.
            let value = unsafe {
                webkit2gtk::ffi::webkit_script_dialog_prompt_get_default_text(
                    native.to_glib_none().0,
                )
            };
            let Some(value) = bounded_native_text(value) else {
                return false;
            };
            Some(value)
        } else {
            None
        };
        if BUSY.with(|busy| busy.replace(true)) {
            return false;
        }
        let lease = Lease;
        let panel = gtk::Dialog::with_buttons(
            Some("This page says"),
            Some(&owner),
            gtk::DialogFlags::MODAL | gtk::DialogFlags::DESTROY_WITH_PARENT,
            &[("OK", gtk::ResponseType::Ok)],
        );
        panel.set_default_size(360, -1);
        panel.set_default_response(gtk::ResponseType::Ok);
        if kind != ScriptDialogType::Alert {
            panel.add_button("Cancel", gtk::ResponseType::Cancel);
        }
        let label = gtk::Label::new(Some(&message));
        label.set_line_wrap(true);
        label.set_max_width_chars(60);
        let content = panel.content_area();
        content.set_border_width(16);
        content.set_spacing(12);
        content.add(&label);
        let entry = default.map(|default| {
            let field = gtk::Entry::new();
            field.set_max_length(1024);
            field.set_activates_default(true);
            field.set_text(&default);
            content.add(&field);
            field
        });
        let suppress = gtk::CheckButton::with_label("Don't allow more dialogs from this page");
        let now = Instant::now();
        let repeat = self
            .last
            .get()
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(10));
        if repeat {
            content.add(&suppress);
        }
        let identity = Rc::new(());
        let weak = Rc::downgrade(self);
        let response_identity = identity.clone();
        panel.connect_response(move |_, response| {
            if let Some(broker) = weak.upgrade() {
                broker.finish(&response_identity, response);
            }
        });
        if !self.current(activity, before_unload) || !page.is_mapped() || !owner.is_active() {
            panel.close();
            return false;
        }
        self.last.set(Some(now));
        *self.pending.borrow_mut() = Some(Pending {
            identity,
            activity,
            before_unload,
            page: page.downgrade(),
            owner,
            panel: panel.clone(),
            entry,
            suppress,
            native: Some(native.clone()),
            _lease: lease,
        });
        panel.show_all();
        true
    }
    fn finish(&self, identity: &Rc<()>, response: gtk::ResponseType) {
        let mut slot = self.pending.borrow_mut();
        if !slot
            .as_ref()
            .is_some_and(|pending| Rc::ptr_eq(&pending.identity, identity))
        {
            return;
        }
        let mut pending = slot.take().unwrap();
        drop(slot);
        self.silenced
            .set(self.silenced.get() || pending.suppress.is_active());
        let admitted = response == gtk::ResponseType::Ok
            && self.current(pending.activity, pending.before_unload)
            && pending.owner.is_visible()
            && pending.page.upgrade().is_some_and(|page| {
                page.is_mapped() && page.toplevel().as_ref() == Some(pending.owner.upcast_ref())
            });
        if let Some(native) = pending.native.take() {
            match native.dialog_type() {
                ScriptDialogType::Confirm | ScriptDialogType::BeforeUnloadConfirm => {
                    native.confirm_set_confirmed(admitted)
                }
                ScriptDialogType::Prompt if admitted => {
                    if let Some(entry) = &pending.entry {
                        native.prompt_set_text(&entry.text());
                    }
                }
                _ => {}
            }
            native.close();
            if pending.before_unload && !admitted {
                self.settle_cancelled_load(pending.activity);
            }
        }
    }
}
impl Drop for ScriptDialogs {
    fn drop(&mut self) {
        drop(self.pending.get_mut().take());
    }
}

fn bounded_native_text(pointer: *const std::ffi::c_char) -> Option<String> {
    if pointer.is_null() {
        return Some(String::new());
    }
    // SAFETY: callers pass WebKit-owned C strings; inspect at most the admission limit.
    let length = unsafe { libc::strnlen(pointer, 4097) };
    if length > 4096 {
        return None;
    }
    // SAFETY: the bounded prefix lies within the live native string.
    let bytes = unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), length) };
    std::str::from_utf8(bytes)
        .ok()
        .map(|text| text.chars().take(1024).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_dialog_strings_are_bounded_and_plain_text() {
        assert_eq!(
            bounded_native_text(c"<b>text</b>".as_ptr()).as_deref(),
            Some("<b>text</b>")
        );
        assert!(
            bounded_native_text(std::ffi::CString::new("x".repeat(4097)).unwrap().as_ptr())
                .is_none()
        );
        assert!(bounded_native_text(c"\xff".as_ptr()).is_none());
    }
}
