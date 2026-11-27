//! `chrome.downloads` for the WebKit extension runtime, answered by the
//! browser's own download manager. A download an extension starts belongs to
//! the focused window's tab, so it meets the same admission, destination
//! choice and dangerous-type hold as one that page starts.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2_foundation::{NSString, NSURLRequest, NSURL};
use objc2_web_kit::WKDownload;
use zephium_core::downloads::{
    DownloadCall, DownloadCompletion, DownloadError, DownloadResponse, DownloadView,
    MAX_DOWNLOAD_PAGE,
};
use zephium_core::ids::{DownloadId, ProfileId};
use zephium_core::permissions::PageOrigin;
use zephium_core::ports::engine::Partition;
use zephium_webext_macos::{DownloadAnswer, DownloadItem, DownloadRequest};

pub(crate) type DownloadDone = Box<dyn FnOnce(Result<DownloadAnswer, String>)>;

/// The profile's downloads numbered for extensions, for this run.
#[derive(Default)]
pub(crate) struct DownloadNumbers {
    forward: HashMap<DownloadId, u64>,
    back: HashMap<u64, DownloadId>,
}

impl DownloadNumbers {
    fn number(&mut self, id: DownloadId) -> u64 {
        let next = self.forward.len() as u64 + 1;
        *self.forward.entry(id).or_insert_with(|| {
            self.back.insert(next, id);
            next
        })
    }

    fn items(&mut self, views: Vec<DownloadView>) -> Vec<DownloadItem> {
        views
            .into_iter()
            .filter_map(|view| {
                let id = DownloadId::parse(&view.id)?;
                Some(DownloadItem {
                    id: self.number(id),
                    view,
                })
            })
            .collect()
    }
}

fn explain(error: DownloadError) -> String {
    match error {
        DownloadError::Invalid => "The download can't do that in its current state.".into(),
        DownloadError::Capacity => "Too many downloads are in progress.".into(),
        DownloadError::MissingFile => "The downloaded file is gone.".into(),
        DownloadError::ChangedFile => "The downloaded file has changed since.".into(),
        error => format!("The download manager refused: {error:?}."),
    }
}

/// Answers on the main thread, whichever thread the manager finishes on.
fn completion(
    numbers: Rc<RefCell<DownloadNumbers>>,
    done: DownloadDone,
    mtm: MainThreadMarker,
) -> DownloadCompletion {
    let bound = dispatch2::MainThreadBound::new((numbers, done), mtm);
    DownloadCompletion::new(move |response| {
        dispatch2::DispatchQueue::main().exec_async(move || {
            let mtm = MainThreadMarker::new().expect("the main queue runs on the main thread");
            let (numbers, done) = bound.into_inner(mtm);
            done(match response {
                DownloadResponse::Page { entries, .. } => {
                    Ok(DownloadAnswer::Items(numbers.borrow_mut().items(entries)))
                }
                DownloadResponse::Accepted | DownloadResponse::Applied => Ok(DownloadAnswer::Done),
                DownloadResponse::Error { error } => Err(explain(error)),
                _ => Err("The download manager answered with something unexpected.".into()),
            });
        });
    })
}

thread_local! {
    /// Profiles whose download change is already on its way to extensions.
    static PENDING: RefCell<HashSet<ProfileId>> = RefCell::new(HashSet::new());
    static WATCHED: Cell<bool> = const { Cell::new(false) };
}

impl super::EngineHost {
    /// Lets extensions hear what the manager reports to the shell, once per
    /// turn of the main loop and only for profiles whose extensions listen.
    pub(crate) fn watch_extension_downloads(&self) {
        if let Some(downloads) = &self.downloads {
            if !WATCHED.with(|watched| watched.replace(true)) {
                watch(downloads);
            }
        }
    }
}

fn watch(downloads: &super::downloads::Downloads) {
    downloads.set_watch(Box::new(|profile| {
        if !PENDING.with(|pending| pending.borrow_mut().insert(profile)) {
            return;
        }
        dispatch2::DispatchQueue::main().exec_async(move || {
            PENDING.with(|pending| pending.borrow_mut().remove(&profile));
            let _ = super::try_with(move |host| host.extension_downloads_changed(profile));
        });
    }));
}

impl super::EngineHost {
    pub(super) fn extension_downloads_changed(&mut self, profile: ProfileId) {
        let Some(downloads) = self.downloads.clone() else {
            return;
        };
        let Some((runtime, numbers)) = self.webext.downloads_runtime(profile) else {
            return;
        };
        if !runtime.hears_downloads() {
            return;
        }
        // Updates answer at once, from memory.
        let answer = Arc::new(Mutex::new(None));
        let slot = answer.clone();
        downloads.call(
            Partition::Persistent(profile),
            DownloadCall::Updates,
            DownloadCompletion::new(move |response| {
                *slot.lock().unwrap_or_else(|error| error.into_inner()) = Some(response);
            }),
        );
        let answer = answer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(DownloadResponse::Updates {
            entries, removed, ..
        }) = answer
        {
            let mut numbers = numbers.borrow_mut();
            let items = numbers.items(entries);
            let erased = removed
                .iter()
                .filter_map(|id| DownloadId::parse(id))
                .filter_map(|id| numbers.forward.get(&id).copied())
                .collect();
            drop(numbers);
            runtime.downloads_changed(items, erased);
        }
    }

    pub(super) fn extension_downloads(
        &mut self,
        profile: ProfileId,
        request: DownloadRequest,
        done: DownloadDone,
    ) {
        let Some(mtm) = MainThreadMarker::new() else {
            return done(Err("Downloads are not available.".into()));
        };
        let (Some(downloads), Some((_, numbers))) = (
            self.downloads.clone(),
            self.webext.downloads_runtime(profile),
        ) else {
            return done(Err("Downloads are not available.".into()));
        };
        let partition = Partition::Persistent(profile);
        let call = |id: u64, make: fn(String) -> DownloadCall| {
            numbers
                .borrow()
                .back
                .get(&id)
                .map(|id| make(id.to_string()))
                .ok_or_else(|| "Invalid downloadId.".to_owned())
        };
        let call = match request {
            DownloadRequest::Start { url, filename } => {
                return self.extension_download_start(profile, url, filename, numbers, done)
            }
            DownloadRequest::List => Ok(DownloadCall::List {
                before: None,
                limit: MAX_DOWNLOAD_PAGE,
            }),
            DownloadRequest::Pause(id) => call(id, |id| DownloadCall::Pause { id }),
            DownloadRequest::Resume(id) => call(id, |id| DownloadCall::Resume { id }),
            DownloadRequest::Cancel(id) => call(id, |id| DownloadCall::Cancel { id }),
            DownloadRequest::Erase(id) => call(id, |id| DownloadCall::Forget { id }),
            DownloadRequest::Open(id) => call(id, |id| DownloadCall::Open { id }),
            DownloadRequest::Show(id) => call(id, |id| DownloadCall::Reveal { id }),
        };
        match call {
            Ok(call) => downloads.call(partition, call, completion(numbers, done, mtm)),
            Err(message) => done(Err(message)),
        }
    }

    fn extension_download_start(
        &mut self,
        profile: ProfileId,
        url: String,
        filename: Option<String>,
        numbers: Rc<RefCell<DownloadNumbers>>,
        done: DownloadDone,
    ) {
        let refused = || {
            Err(
                "A download starts only in the window in front, from a page that can download."
                    .to_owned(),
            )
        };
        let item = self
            .extension_browser_surfaces
            .get(&profile)
            .and_then(|surface| {
                let focused = surface.focused()?;
                surface
                    .windows()
                    .iter()
                    .find(|window| window.id() == focused && !window.is_private())?
                    .active()
            })
            .filter(|item| {
                self.partitions
                    .get(item)
                    .is_some_and(|partition| *partition == Partition::Persistent(profile))
            });
        let (Some(item), Some(downloads)) = (item, self.downloads.clone()) else {
            return done(refused());
        };
        let Some(observed) = self.views.get(&item) else {
            return done(refused());
        };
        let (Some(address), Ok(parsed)) = (
            NSURL::URLWithString(&NSString::from_str(&url)),
            url::Url::parse(&url),
        ) else {
            return done(Err("Invalid URL".into()));
        };
        // A web address is shown as where the file came from; generated data
        // is shown in the context of the page it was made in.
        let origin = match parsed.scheme() {
            "data" => None,
            _ => match PageOrigin::from_url(&parsed) {
                Ok(origin) => Some(origin),
                Err(_) => return done(Err("Invalid URL".into())),
            },
        };
        let webview = crate::platform::imp::native_webview(&observed.view);
        let permit = observed.event_permit.clone();
        let intent = observed.download_surface_intent.clone();
        let navigation = observed.navigation.clone();
        let request = NSURLRequest::requestWithURL(&address);
        let weak = Rc::downgrade(&downloads);
        let reply = RefCell::new(Some((done, numbers, filename, origin)));
        let started = RcBlock::new(move |native: NonNull<WKDownload>| {
            let Some((done, numbers, filename, origin)) = reply.take() else {
                return;
            };
            let native = unsafe { native.as_ref() };
            let Some(downloads) = weak.upgrade() else {
                unsafe { native.cancel(None) };
                return done(Err("Downloads are not available.".into()));
            };
            match downloads.admit_for_extension(
                Partition::Persistent(profile),
                permit.clone(),
                intent.clone(),
                navigation.clone(),
                native,
            ) {
                Some(id) => {
                    downloads.extension_started(id, filename, origin);
                    done(Ok(DownloadAnswer::Started(numbers.borrow_mut().number(id))))
                }
                None => done(refused()),
            }
        });
        unsafe { webview.startDownloadUsingRequest_completionHandler(&request, &started) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_keeps_its_number_for_the_run() {
        let mut numbers = DownloadNumbers::default();
        let (a, b) = (DownloadId::generate(), DownloadId::generate());
        assert_eq!(
            (numbers.number(a), numbers.number(b), numbers.number(a)),
            (1, 2, 1)
        );
        assert_eq!(numbers.back.get(&2), Some(&b));
    }
}
