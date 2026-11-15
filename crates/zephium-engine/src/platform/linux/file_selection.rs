use gtk::glib::translate::ToGlibPtr;
use gtk::prelude::*;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use webkit2gtk::WebViewExt;

unsafe extern "C" {
    fn webkit_settings_get_all_features() -> *mut c_void;
    fn webkit_feature_list_find(list: *mut c_void, name: *const std::ffi::c_char) -> *mut c_void;
    fn webkit_feature_list_unref(list: *mut c_void);
    fn webkit_settings_set_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
        enabled: i32,
    );
    fn webkit_settings_get_feature_enabled(
        settings: *mut webkit2gtk::ffi::WebKitSettings,
        feature: *mut c_void,
    ) -> i32;
}

pub(super) fn disable_directories(view: &webkit2gtk::WebView) -> Result<(), String> {
    let settings = WebViewExt::settings(view).ok_or("missing file-selection settings")?;
    // These public 2.42 APIs are newer than the Rust binding. Runtime admission
    // requires a newer WebKit; the list owns the borrowed feature through use.
    let disabled = unsafe {
        let list = webkit_settings_get_all_features();
        if list.is_null() {
            return Err("missing WebKit feature list".into());
        }
        let feature = webkit_feature_list_find(list, c"DirectoryUpload".as_ptr());
        let disabled = if feature.is_null() {
            false
        } else {
            webkit_settings_set_feature_enabled(settings.to_glib_none().0, feature, 0);
            webkit_settings_get_feature_enabled(settings.to_glib_none().0, feature) == 0
        };
        webkit_feature_list_unref(list);
        disabled
    };
    if disabled {
        Ok(())
    } else {
        Err("directory upload could not be disabled".into())
    }
}

pub(crate) struct CheckedFile {
    path: PathBuf,
    file: File,
    pub(crate) encoded: String,
}
impl CheckedFile {
    pub(crate) fn open(selected: &Path) -> Option<Self> {
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
            .custom_flags(libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC)
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
    pub(crate) fn still_matches(&self) -> bool {
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

pub(super) fn filter_dropped_files(view: &webkit2gtk::WebView) {
    let forwarding = std::rc::Rc::new(std::cell::Cell::new(false));
    view.connect_drag_data_received(move |view, context, x, y, data, info, time| {
        if forwarding.get() {
            return;
        }
        if data.target().name().as_str() != "text/uri-list" {
            return;
        }
        let files = (data.length() > 0 && data.length() <= 1024 * 4096).then(|| data.uris());
        if files
            .as_ref()
            .is_some_and(|uris| !uris.iter().any(|uri| uri.starts_with("file:")))
        {
            return;
        }
        let checked = files
            .filter(|uris| !uris.is_empty() && uris.len() <= 1024)
            .and_then(|uris| {
                uris.iter()
                    .map(|uri| {
                        let path = gtk::gio::File::for_uri(uri).path()?;
                        CheckedFile::open(&path)
                    })
                    .collect::<Option<Vec<_>>>()
            });
        // Received selection storage is owned by GTK. Mutating it in place
        // double-frees GTK's buffer; forward an owned copy instead.
        let filtered = data.clone();
        if let Some(files) = checked.filter(|files| files.iter().all(CheckedFile::still_matches)) {
            let uris: Vec<_> = files
                .iter()
                .map(|file| gtk::gio::File::for_path(&file.path).uri())
                .collect();
            let borrowed: Vec<_> = uris.iter().map(|uri| uri.as_str()).collect();
            filtered.set_uris(&borrowed);
        } else {
            // WebKit's after-handler must consume an empty answer so its drag
            // transaction drains; stopping the signal leaves pending data behind.
            filtered.set(&data.target(), 8, &[]);
        }
        view.stop_signal_emission_by_name("drag-data-received");
        forwarding.set(true);
        view.emit_by_name::<()>(
            "drag-data-received",
            &[context, &x, &y, &filtered, &info, &time],
        );
        forwarding.set(false);
    });
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
