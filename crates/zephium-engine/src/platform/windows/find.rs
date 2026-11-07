//! Find in page through WebView2's Find API with its own dialog suppressed:
//! chrome draws the field, WebView2 highlights and counts. Runtimes older than
//! the API refuse the search so the host can answer it.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use webview2_com::Microsoft::Web::WebView2::Win32::{
    ICoreWebView2, ICoreWebView2Environment15, ICoreWebView2Find, ICoreWebView2_2, ICoreWebView2_28,
};
use webview2_com::{
    FindActiveMatchIndexChangedEventHandler, FindMatchCountChangedEventHandler,
    FindStartCompletedHandler,
};
use windows_core::{Interface, HSTRING};
use zephium_core::ports::engine::FindRequest;

/// Reports `(query, matches, one-based active match)`.
pub(crate) type FindReport = Box<dyn Fn(String, u32, Option<u32>)>;

/// One view's find session; dropping it stops the search and its handlers.
pub(crate) struct FindSession {
    find: ICoreWebView2Find,
    environment: ICoreWebView2Environment15,
    query: Rc<RefCell<String>>,
    generation: Rc<Cell<u64>>,
    report: Rc<FindReport>,
    tokens: [i64; 2],
}

impl Drop for FindSession {
    fn drop(&mut self) {
        self.query.borrow_mut().clear();
        self.generation.set(0);
        unsafe {
            let _ = self.find.remove_MatchCountChanged(self.tokens[0]);
            let _ = self.find.remove_ActiveMatchIndexChanged(self.tokens[1]);
            let _ = self.find.Stop();
        }
    }
}

fn state(find: &ICoreWebView2Find) -> (u32, Option<u32>) {
    let mut matches = 0i32;
    let mut active = -1i32;
    unsafe {
        let _ = find.MatchCount(&mut matches);
        let _ = find.ActiveMatchIndex(&mut active);
    }
    let matches = u32::try_from(matches).unwrap_or(0);
    let active = u32::try_from(active)
        .ok()
        .map(|index| index.saturating_add(1));
    (matches, active.filter(|_| matches > 0))
}

fn open(core: &ICoreWebView2, report: FindReport) -> windows_core::Result<FindSession> {
    let find = unsafe { core.cast::<ICoreWebView2_28>()?.Find()? };
    let environment = unsafe {
        core.cast::<ICoreWebView2_2>()?
            .Environment()?
            .cast::<ICoreWebView2Environment15>()?
    };
    let query = Rc::new(RefCell::new(String::new()));
    let report = Rc::new(report);
    let mut tokens = [0i64; 2];
    let (counted_query, counted_report) = (query.clone(), report.clone());
    let counted = FindMatchCountChangedEventHandler::create(Box::new(move |find, _| {
        if let Some(find) = find {
            let (matches, active) = state(&find);
            counted_report(counted_query.borrow().clone(), matches, active);
        }
        Ok(())
    }));
    let (moved_query, moved_report) = (query.clone(), report.clone());
    let moved = FindActiveMatchIndexChangedEventHandler::create(Box::new(move |find, _| {
        if let Some(find) = find {
            let (matches, active) = state(&find);
            moved_report(moved_query.borrow().clone(), matches, active);
        }
        Ok(())
    }));
    unsafe {
        find.add_MatchCountChanged(&counted, &mut tokens[0])?;
        find.add_ActiveMatchIndexChanged(&moved, &mut tokens[1])?;
    }
    Ok(FindSession {
        find,
        environment,
        query,
        generation: Rc::new(Cell::new(0)),
        report,
        tokens,
    })
}

/// Runs one find step. Returns false when this runtime cannot find.
pub(crate) fn find(
    core: &ICoreWebView2,
    session: &mut Option<FindSession>,
    request: Option<&FindRequest>,
    report: impl FnOnce() -> FindReport,
) -> bool {
    let Some(request) = request else {
        *session = None;
        return true;
    };
    if session.is_none() {
        match open(core, report()) {
            Ok(opened) => *session = Some(opened),
            Err(_) => return false,
        }
    }
    let Some(active) = session.as_ref() else {
        return false;
    };
    let repeat = *active.query.borrow() == request.query;
    let stepped = unsafe {
        if repeat && request.forward {
            active.find.FindNext()
        } else if repeat {
            active.find.FindPrevious()
        } else {
            let Some(generation) = active.generation.get().checked_add(1) else {
                return false;
            };
            active.generation.set(generation);
            *active.query.borrow_mut() = request.query.clone();
            active
                .environment
                .CreateFindOptions()
                .and_then(|options| {
                    options.SetFindTerm(&HSTRING::from(request.query.as_str()))?;
                    options.SetIsCaseSensitive(false)?;
                    options.SetShouldHighlightAllMatches(true)?;
                    options.SetSuppressDefaultFindDialog(true)?;
                    Ok(options)
                })
                .and_then(|options| {
                    let query = active.query.clone();
                    let expected = request.query.clone();
                    let report = active.report.clone();
                    let current = active.generation.clone();
                    active.find.Start(
                        &options,
                        &FindStartCompletedHandler::create(Box::new(move |result| {
                            if result.is_err()
                                && current.get() == generation
                                && *query.borrow() == expected
                            {
                                query.borrow_mut().clear();
                                report(expected.clone(), 0, None);
                            }
                            Ok(())
                        })),
                    )
                })
        }
    };
    if stepped.is_err() {
        active.query.borrow_mut().clear();
    }
    stepped.is_ok()
}
