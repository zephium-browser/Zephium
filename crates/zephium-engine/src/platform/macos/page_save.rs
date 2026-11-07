//! Save Page As: the page as a web archive, or its source as HTML, written
//! where the person chooses. The file is named by what they type; an `.html`
//! name saves the source, anything else the archive.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSArray, NSData, NSError, NSString};
use objc2_web_kit::WKWebView;

const ARCHIVE: &str = "webarchive";
const SOURCE: &str = "html";
/// An archive holds every image and script of the page; refuse one that would
/// not fit in memory twice over.
const MAX_ARCHIVE_BYTES: usize = 512 * 1024 * 1024;

/// A file name from a page title: one line, nothing a file system rejects.
fn file_stem(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches('.');
    let stem: String = trimmed.chars().take(80).collect();
    if stem.is_empty() {
        "Untitled".into()
    } else {
        stem
    }
}

/// Which file a name asks for, once the panel has settled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Archive,
    Source,
}

fn format_of(path: &std::path::Path, has_source: bool) -> Format {
    let source = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "html" | "htm"));
    if source && has_source {
        Format::Source
    } else {
        Format::Archive
    }
}

/// Starts saving `page`. `false` means no panel could be shown. The archive
/// is made first and the panel follows, so a name is only asked for when
/// there is something to write. `source` is the page's HTML if it has been
/// read by the time the archive is ready.
pub(crate) fn save_page(page: Retained<WKWebView>, source: Arc<Mutex<Option<String>>>) -> bool {
    let Some(window) = page.window() else {
        return false;
    };
    if !window.isKeyWindow() || window.attachedSheet().is_some() {
        return false;
    }
    let stem = file_stem(
        &unsafe { page.title() }
            .map(|title| title.to_string())
            .unwrap_or_default(),
    );
    let host = page.clone();
    let archived = RcBlock::new(move |data: *mut NSData, _error: *mut NSError| {
        // SAFETY: WebKit passes an autoreleased NSData or null, on the main thread.
        let Some(data) = (unsafe { Retained::retain(data) }) else {
            return;
        };
        if data.len() > MAX_ARCHIVE_BYTES {
            return;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(window) = host.window() else {
            return;
        };
        if window.attachedSheet().is_some() {
            return;
        }
        let source = Arc::new(source.lock().ok().and_then(|mut source| source.take()));
        let panel = NSSavePanel::savePanel(mtm);
        panel.setNameFieldStringValue(&NSString::from_str(&format!("{stem}.{ARCHIVE}")));
        panel.setCanCreateDirectories(true);
        panel.setExtensionHidden(false);
        let kinds: Vec<Retained<NSString>> = [ARCHIVE]
            .into_iter()
            .chain(source.is_some().then_some(SOURCE))
            .map(NSString::from_str)
            .collect();
        #[allow(deprecated)]
        panel.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&kinds)));
        let chosen = panel.clone();
        let source = source.clone();
        let done = RcBlock::new(move |result: NSModalResponse| {
            let path = (result == NSModalResponseOK)
                .then(|| chosen.URL())
                .flatten()
                .and_then(|url| url.path())
                .map(|path| PathBuf::from(path.to_string()));
            chosen.orderOut(None);
            let Some(path) = path else {
                return;
            };
            let bytes = match (format_of(&path, source.is_some()), source.as_deref()) {
                (Format::Source, Some(html)) => html.as_bytes().to_vec(),
                _ => data.to_vec(),
            };
            // Writing can be large; the main thread has a page to draw.
            let _ = std::thread::Builder::new()
                .name("save-page".into())
                .spawn(move || {
                    if let Err(error) = std::fs::write(&path, bytes) {
                        eprintln!("engine: could not save the page: {}", error.kind());
                    }
                });
        });
        panel.beginSheetModalForWindow_completionHandler(&window, &done);
    });
    unsafe { page.createWebArchiveDataWithCompletionHandler(&archived) };
    true
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn a_title_becomes_one_plain_file_name() {
        assert_eq!(
            file_stem("Docs: the \"guide\" / part 1?"),
            "Docs the guide part 1"
        );
        assert_eq!(file_stem("  ..hidden.. \n"), "hidden");
        assert_eq!(file_stem(""), "Untitled");
        assert_eq!(file_stem("///"), "Untitled");
        assert_eq!(file_stem(&"w".repeat(300)).chars().count(), 80);
        assert_eq!(file_stem("日本語のページ"), "日本語のページ");
    }

    #[test]
    fn the_extension_chosen_decides_between_the_archive_and_the_source() {
        let archive = |name: &str, has_source| format_of(Path::new(name), has_source);
        assert_eq!(archive("/tmp/page.webarchive", true), Format::Archive);
        assert_eq!(archive("/tmp/page.html", true), Format::Source);
        assert_eq!(archive("/tmp/page.HTM", true), Format::Source);
        // Without a source to write, any name saves the archive.
        assert_eq!(archive("/tmp/page.html", false), Format::Archive);
        assert_eq!(archive("/tmp/page", true), Format::Archive);
    }
}
