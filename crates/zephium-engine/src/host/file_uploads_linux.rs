//! Tab-owned GTK file selection; native consent supplies the only disclosed paths.
use super::permits::EventPermit;
use crate::navigation_epoch::{NavigationActivity, NavigationEpochTracker};
use gtk::{glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use webkit2gtk::{FileChooserRequest, FileChooserRequestExt, WebView};

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
    page: glib::WeakRef<WebView>,
    owner: gtk::Window,
    panel: gtk::FileChooserNative,
    request: Option<FileChooserRequest>,
    multiple: bool,
    _lease: Lease,
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.panel.hide();
        self.panel.destroy();
        if let Some(request) = self.request.take() {
            request.cancel();
        }
    }
}
pub(super) struct FileUploadBroker {
    permit: EventPermit,
    navigation: NavigationEpochTracker,
    presentation: Arc<AtomicBool>,
    pending: RefCell<Option<Pending>>,
}
impl FileUploadBroker {
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
        })
    }
    pub(super) fn has_pending(&self) -> bool {
        self.pending.borrow().is_some()
    }
    pub(super) fn cancel(&self) {
        let pending = self.pending.borrow_mut().take();
        drop(pending);
    }
    fn current(&self, activity: NavigationActivity) -> bool {
        self.permit.active_token().is_some()
            && self.navigation.matches_activity(activity)
            && self.navigation.current_committed().is_some()
            && self.presentation.load(Ordering::Acquire)
    }
    pub(super) fn present(self: &Rc<Self>, page: &WebView, request: &FileChooserRequest) -> bool {
        let Some(activity) = self.navigation.activity_snapshot() else {
            return false;
        };
        let Some(owner) = page
            .toplevel()
            .and_then(|w| w.downcast::<gtk::Window>().ok())
        else {
            return false;
        };
        if !self.current(activity)
            || !page.is_mapped()
            || !owner.is_active()
            || self.pending.borrow().is_some()
        {
            return false;
        }
        let Some(origin) = self
            .navigation
            .committed_snapshot()
            .and_then(|(_, url)| url::Url::parse(&url).ok())
            .and_then(|url| zephium_core::permissions::PageOrigin::from_url(&url).ok())
        else {
            return false;
        };
        if BUSY.with(|busy| busy.replace(true)) {
            return false;
        }
        let panel = gtk::FileChooserNative::new(
            Some(&format!("Choose files for {}", origin.as_str())),
            Some(&owner),
            gtk::FileChooserAction::Open,
            Some("Open"),
            Some("Cancel"),
        );
        panel.set_select_multiple(request.selects_multiple());
        panel.set_local_only(true);
        panel.set_create_folders(false);
        if let Some(filter) = request.mime_types_filter() {
            panel.add_filter(filter);
        }
        let identity = Rc::new(());
        let response_identity = identity.clone();
        let weak = Rc::downgrade(self);
        panel.connect_response(move |_, response| {
            if let Some(broker) = weak.upgrade() {
                broker.finish(&response_identity, response);
            }
        });
        *self.pending.borrow_mut() = Some(Pending {
            identity,
            activity,
            page: page.downgrade(),
            owner,
            panel: panel.clone(),
            request: Some(request.clone()),
            multiple: request.selects_multiple(),
            _lease: Lease,
        });
        panel.show();
        true
    }
    fn finish(&self, identity: &Rc<()>, response: gtk::ResponseType) {
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
            || !pending.owner.is_visible()
            || !pending.page.upgrade().is_some_and(|p| {
                p.is_mapped() && p.toplevel().as_ref() == Some(pending.owner.upcast_ref())
            })
        {
            return;
        }
        let files = pending.panel.filenames();
        if files.is_empty() || files.len() > 1024 || (!pending.multiple && files.len() != 1) {
            return;
        }
        let Some(files) = files
            .iter()
            .map(|path| CheckedFile::open(path))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        // WebKit accepts paths, not descriptors. Recheck the named inode immediately
        // before handing it over, retaining the no-follow descriptors through the call.
        if !self.current(pending.activity) || files.iter().any(|file| !file.still_matches()) {
            return;
        }
        let paths: Vec<_> = files.iter().map(|file| file.encoded.as_str()).collect();
        if let Some(request) = pending.request.take() {
            request.select_files(&paths);
        }
    }
    #[cfg(test)]
    pub(super) fn test_panel(&self) -> Option<gtk::FileChooserNative> {
        self.pending.borrow().as_ref().map(|p| p.panel.clone())
    }
}
impl Drop for FileUploadBroker {
    fn drop(&mut self) {
        drop(self.pending.get_mut().take());
    }
}

struct CheckedFile {
    path: PathBuf,
    file: File,
    encoded: String,
}
impl CheckedFile {
    fn open(selected: &Path) -> Option<Self> {
        if !selected.is_absolute() || selected.as_os_str().len() > 4096 {
            return None;
        }
        let path = selected.canonicalize().ok()?;
        let text = path.to_str()?;
        if text.len() > 4096 || text.contains('\0') {
            return None;
        }
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&path)
            .ok()?;
        if !file.metadata().ok()?.is_file() {
            return None;
        }
        // select_files decodes URI escapes even though its argument is a filename.
        let encoded = text.replace('%', "%25");
        Some(Self {
            path,
            file,
            encoded,
        })
    }
    fn still_matches(&self) -> bool {
        use std::os::unix::fs::MetadataExt;
        let Ok(held) = self.file.metadata() else {
            return false;
        };
        let Ok(named) = self.path.symlink_metadata() else {
            return false;
        };
        named.is_file() && held.is_file() && held.dev() == named.dev() && held.ino() == named.ino()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::{ffi::OsStringExt, fs::symlink, net::UnixListener};
    #[test]
    fn only_regular_files_and_resolved_regular_symlinks_are_accepted() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("100%20 space\n.txt");
        std::fs::write(&file, "chosen").unwrap();
        let checked = CheckedFile::open(&file).unwrap();
        assert!(checked.encoded.ends_with("100%2520 space\n.txt"));
        assert!(checked.still_matches());
        let link = dir.path().join("link");
        symlink(&file, &link).unwrap();
        assert!(CheckedFile::open(&link).unwrap().still_matches());
        assert!(CheckedFile::open(dir.path()).is_none());
        let directory_link = dir.path().join("directory-link");
        symlink(dir.path(), &directory_link).unwrap();
        assert!(CheckedFile::open(&directory_link).is_none());
        assert!(CheckedFile::open(Path::new("/dev/null")).is_none());
        let socket = dir.path().join("socket");
        let _listener = UnixListener::bind(&socket).unwrap();
        assert!(CheckedFile::open(&socket).is_none());
        let fifo = dir.path().join("fifo");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        assert!(CheckedFile::open(&fifo).is_none());
        let non_utf8 = dir.path().join(std::ffi::OsString::from_vec(vec![0xff]));
        std::fs::write(&non_utf8, "invalid path").unwrap();
        assert!(CheckedFile::open(&non_utf8).is_none());
        std::fs::remove_file(&file).unwrap();
        symlink(dir.path(), &file).unwrap();
        assert!(!checked.still_matches());
    }
}
