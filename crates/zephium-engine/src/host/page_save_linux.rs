//! Save the committed main resource through download publication safeguards.
use super::{download_files::Destination, permits::EventPermit};
use crate::navigation_epoch::{NavigationActivity, NavigationEpochTracker};
use glib::translate::ToGlibPtr;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use webkit2gtk::{URIResponseExt, WebResource, WebResourceExt, WebView, WebViewExt};
use zephium_core::{downloads::DownloadError, ids::DownloadId};

const MAX_BYTES: usize = 64 * 1024 * 1024;
static BUSY: AtomicBool = AtomicBool::new(false);
struct Lease;
impl Drop for Lease {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::Release);
    }
}
struct Pending {
    identity: Rc<()>,
    activity: NavigationActivity,
    page: glib::WeakRef<WebView>,
    panel: gtk::FileChooserNative,
    resource: WebResource,
    source: String,
    _lease: Option<Lease>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.panel.hide();
        self.panel.destroy();
    }
}
pub(super) struct PageSave {
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    presentation: Arc<AtomicBool>,
    pending: RefCell<Option<Pending>>,
    read: RefCell<Option<gio::Cancellable>>,
}
impl PageSave {
    pub(super) fn new(
        permit: EventPermit,
        navigation: NavigationEpochTracker,
        presentation: Arc<AtomicBool>,
    ) -> Rc<Self> {
        Rc::new(Self {
            permit,
            navigation,
            presentation,
            pending: RefCell::new(None),
            read: RefCell::new(None),
        })
    }
    pub(super) fn cancel(&self) {
        drop(self.pending.borrow_mut().take());
        if let Some(read) = self.read.borrow_mut().take() {
            read.cancel();
        }
    }
    fn current(&self, activity: NavigationActivity) -> bool {
        self.permit.active_token().is_some()
            && self.navigation.matches_activity(activity)
            && self.navigation.current_committed().is_some()
            && self.presentation.load(Ordering::Acquire)
    }
    pub(super) fn present(self: &Rc<Self>, page: &WebView) {
        let Some(activity) = self.navigation.activity_snapshot() else {
            return;
        };
        let Some(owner) = page
            .toplevel()
            .and_then(|w| w.downcast::<gtk::Window>().ok())
        else {
            return;
        };
        let Some(resource) = page.main_resource() else {
            return;
        };
        let Some((_, source)) = self.navigation.committed_snapshot() else {
            return;
        };
        if !self.current(activity)
            || !page.is_mapped()
            || !owner.is_active()
            || resource
                .response()
                .is_some_and(|r| r.content_length() > MAX_BYTES as u64)
            || BUSY.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let panel = gtk::FileChooserNative::new(
            Some("Save Page As"),
            Some(&owner),
            gtk::FileChooserAction::Save,
            Some("Save"),
            Some("Cancel"),
        );
        panel.set_local_only(true);
        let filename = resource
            .response()
            .and_then(|response| response.suggested_filename())
            .filter(|name| zephium_core::downloads::valid_filename(name))
            .map(|name| name.to_string())
            .unwrap_or_else(|| "page.html".into());
        panel.set_current_name(&filename);
        let identity = Rc::new(());
        let response_id = identity.clone();
        let weak = Rc::downgrade(self);
        panel.connect_response(move |_, response| {
            if let Some(broker) = weak.upgrade() {
                broker.finish(&response_id, response);
            }
        });
        *self.pending.borrow_mut() = Some(Pending {
            identity,
            activity,
            page: page.downgrade(),
            panel: panel.clone(),
            resource,
            source,
            _lease: Some(Lease),
        });
        panel.show();
    }
    fn finish(self: &Rc<Self>, identity: &Rc<()>, response: gtk::ResponseType) {
        let mut slot = self.pending.borrow_mut();
        if !slot
            .as_ref()
            .is_some_and(|p| Rc::ptr_eq(&p.identity, identity))
        {
            return;
        }
        let mut pending = slot.take().unwrap();
        drop(slot);
        if response != gtk::ResponseType::Accept
            || !self.current(pending.activity)
            || !pending.page.upgrade().is_some_and(|p| p.is_mapped())
        {
            return;
        }
        let Some(path) = pending.panel.filename() else {
            return;
        };
        pending.panel.hide();
        let cancellable = gio::Cancellable::new();
        *self.read.borrow_mut() = Some(cancellable.clone());
        let weak = Rc::downgrade(self);
        read_bounded(&pending.resource.clone(), &cancellable, move |bytes| {
            let Some(broker) = weak.upgrade() else {
                return;
            };
            broker.read.borrow_mut().take();
            if !broker.current(pending.activity) {
                return;
            }
            let Some(bytes) = bytes else {
                eprintln!("engine: page source could not be saved within the size limit");
                return;
            };
            let source = pending.source.clone();
            // Transfer the single global work slot only after document validation.
            let lease = pending._lease.take();
            let result = std::thread::Builder::new()
                .name("save-page".into())
                .spawn(move || {
                    let _lease = lease;
                    if let Err(error) = publish(path, bytes, &source) {
                        eprintln!("engine: page save failed: {error:?}");
                    }
                });
            if result.is_err() {
                eprintln!("engine: page save worker unavailable");
            }
        });
    }
    #[cfg(test)]
    pub(super) fn test_panel(&self) -> Option<gtk::FileChooserNative> {
        self.pending.borrow().as_ref().map(|p| p.panel.clone())
    }
}
impl Drop for PageSave {
    fn drop(&mut self) {
        self.cancel();
    }
}
fn publish(path: std::path::PathBuf, bytes: Vec<u8>, source: &str) -> Result<(), DownloadError> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut destination = Destination::prepare(DownloadId::generate(), path, None)?;
    destination.set_provenance(None);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(destination.payload())
        .map_err(|_| DownloadError::Destination)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| DownloadError::Destination)?;
    destination.finish(source, Some(bytes.len() as u64))?;
    Ok(())
}
fn read_bounded(
    resource: &WebResource,
    cancellable: &gio::Cancellable,
    done: impl FnOnce(Option<Vec<u8>>) + 'static,
) {
    type Callback = Box<dyn FnOnce(Option<Vec<u8>>)>;
    unsafe extern "C" fn finish(
        object: *mut glib::gobject_ffi::GObject,
        result: *mut gio::ffi::GAsyncResult,
        data: *mut libc::c_void,
    ) {
        // WebKit invokes on the initiating main context; take its buffer exactly once.
        let done = unsafe { Box::from_raw(data.cast::<Callback>()) };
        let mut length = 0;
        let mut error = std::ptr::null_mut();
        let buffer = unsafe {
            webkit2gtk::ffi::webkit_web_resource_get_data_finish(
                object.cast(),
                result,
                &mut length,
                &mut error,
            )
        };
        let bytes = if error.is_null() && !buffer.is_null() && length <= MAX_BYTES {
            Some(unsafe { std::slice::from_raw_parts(buffer, length) }.to_vec())
        } else {
            None
        };
        unsafe {
            glib::ffi::g_free(buffer.cast());
            if !error.is_null() {
                glib::ffi::g_error_free(error);
            }
        }
        done(bytes);
    }
    let callback: Box<Callback> = Box::new(Box::new(done));
    // Check length before Rust allocation; cancellation still releases the native result.
    unsafe {
        webkit2gtk::ffi::webkit_web_resource_get_data(
            resource.to_glib_none().0,
            cancellable.to_glib_none().0,
            Some(finish),
            Box::into_raw(callback).cast(),
        );
    }
}
