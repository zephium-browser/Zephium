use zephium_core::ids::ItemId;
use zephium_core::ports::engine::{
    EngineEvent, FindRequest, HistoryEntries, HistoryRequestId, ZoomRequestId,
};

use super::scripts::{EXTRACT_HTML_JS, MAX_HTML_CHARS, MAX_HTML_RESULT_BYTES};
use super::EngineHost;

fn settled_zoom_scale(previous: f64, requested: f64, succeeded: bool) -> f64 {
    if succeeded {
        requested
    } else {
        previous
    }
}

impl EngineHost {
    pub(crate) fn zoom(&mut self, id: ItemId, scale: f64, request: ZoomRequestId) {
        let Some((permit, applied_scale, succeeded)) = self.views.get_mut(&id).map(|view| {
            let succeeded = view.zoom(scale).is_ok();
            let applied = settled_zoom_scale(view.applied_zoom.get(), scale, succeeded);
            view.applied_zoom.set(applied);
            (view.event_permit.clone(), applied, succeeded)
        }) else {
            return;
        };
        if !succeeded {
            eprintln!("engine: native zoom invocation failed");
        }
        permit.emit(
            &self.sink,
            EngineEvent::ZoomSettled {
                id,
                request,
                applied_scale,
                succeeded,
            },
        );
    }

    /// Reads the view's back/forward list and reports it as
    /// `EngineEvent::HistoryEntries`. A list that cannot be read is reported
    /// empty, so the caller's request never hangs.
    pub(crate) fn history_entries(&self, id: ItemId, request: HistoryRequestId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let (sink, permit) = (self.sink.clone(), view.event_permit.clone());
        let report = move |session: Option<(Vec<(String, String)>, usize)>| {
            let entries = session
                .map(|(session, current)| HistoryEntries::from_session(&session, current))
                .unwrap_or_default();
            permit.emit(
                &sink,
                EngineEvent::HistoryEntries {
                    id,
                    request,
                    entries,
                },
            );
        };
        let (sink, permit) = (self.sink.clone(), view.event_permit.clone());
        let refused = move || {
            permit.emit(
                &sink,
                EngineEvent::HistoryEntries {
                    id,
                    request,
                    entries: HistoryEntries::default(),
                },
            );
        };
        if !crate::platform::imp::history_session(view, report) {
            refused();
        }
    }

    pub(crate) fn go_to_history_entry(&self, id: ItemId, offset: i32, url: &str) {
        if let Some(view) = self.views.get(&id) {
            view.navigation.release_auth_cleanup();
            if !crate::platform::imp::go_to_history_entry(view, offset, url) {
                eprintln!("engine: history entry is no longer there");
            }
        }
    }

    pub(crate) fn extract_html(&self, id: ItemId) {
        let Some(permit) = self.views.get(&id).map(|view| view.event_permit.clone()) else {
            return;
        };
        let sink = self.sink.clone();
        self.read_html(id, move |read| {
            if let Some((html, truncated)) = read {
                permit.emit(
                    &sink,
                    EngineEvent::HtmlExtracted {
                        id,
                        html,
                        truncated,
                    },
                );
            }
        });
    }

    /// Reads the serialized document of the committed page and hands it to
    /// `done` with whether it was cut, or `None` when the page gave nothing
    /// usable. `done` never runs for a page that has since navigated.
    fn read_html(&self, id: ItemId, done: impl FnOnce(Option<(String, bool)>) + Send + 'static) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let Some(epoch) = view.navigation.current_committed() else {
            return;
        };
        let navigation = view.navigation.clone();
        let script = EXTRACT_HTML_JS.replace("__MAX__", &MAX_HTML_CHARS.to_string());
        let done = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
        let on_result = done.clone();
        let started = view.evaluate_script_with_callback(&script, move |result| {
            if !navigation.is_current(epoch) {
                return;
            }
            let read = (result.len() <= MAX_HTML_RESULT_BYTES)
                .then(|| serde_json::from_str::<String>(&result).ok())
                .flatten()
                .and_then(|value| {
                    let (truncated, html) = if let Some(html) = value.strip_prefix('0') {
                        (false, html)
                    } else {
                        (true, value.strip_prefix('1')?)
                    };
                    (html.encode_utf16().count() <= MAX_HTML_CHARS)
                        .then(|| (html.to_owned(), truncated))
                });
            if let Some(done) = on_result.lock().ok().and_then(|mut done| done.take()) {
                done(read);
            }
        });
        if started.is_err() {
            if let Some(done) = done.lock().ok().and_then(|mut done| done.take()) {
                done(None);
            }
        }
    }

    /// Saves the page through the platform's own Save As. On macOS the page's
    /// source is read first, so the panel can offer it beside the web
    /// archive; a page that gives none still saves as an archive.
    pub(crate) fn save_page(&self, id: ItemId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        #[cfg(target_os = "macos")]
        {
            // The source is read beside the archive, which takes longer; the
            // panel offers it if it has arrived by then.
            let source = std::sync::Arc::new(std::sync::Mutex::new(None));
            let slot = source.clone();
            self.read_html(id, move |read| {
                if let Ok(mut slot) = slot.lock() {
                    *slot = read.map(|(html, _)| html);
                }
            });
            let page = crate::platform::imp::native_webview(view);
            if !crate::platform::macos::page_save::save_page(page, source) {
                eprintln!("engine: Save As is unavailable here");
            }
        }
        #[cfg(target_os = "windows")]
        if let Err(error) = {
            use wry::WebViewExtWindows;
            view.show_save_as()
        } {
            eprintln!("engine: Save As is unavailable: {error}");
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = view;
    }

    /// Run-owned agent pages are never in `views`, so they never get the
    /// panel.
    #[cfg(target_os = "macos")]
    pub(crate) fn autofill_password(&self, id: ItemId) {
        let Some(view) = self.views.get(&id) else {
            return;
        };
        let page = crate::platform::imp::native_webview(view);
        if !crate::platform::macos::autofill::show_passwords(&page) {
            eprintln!("engine: AutoFill has no focused field here");
        }
    }

    /// One step of finding text in `id`. Results travel the view's own event
    /// permit, so a result from a view since replaced is never delivered.
    pub(crate) fn find(&mut self, id: ItemId, request: Option<FindRequest>) {
        let Some(view) = self.views.get_mut(&id) else {
            return;
        };
        let sink = self.sink.clone();
        let permit = view.event_permit.clone();
        let report = move || -> crate::platform::imp::FindReport {
            Box::new(move |query, matches, active| {
                permit.emit(
                    &sink,
                    EngineEvent::FindResult {
                        id,
                        query,
                        matches,
                        active,
                    },
                );
            })
        };
        #[cfg(target_os = "macos")]
        let native = {
            use wry::WebViewExtMacOS;
            view.view.webview()
        };
        #[cfg(target_os = "windows")]
        let native = {
            use wry::WebViewExtWindows;
            view.view.webview()
        };
        #[cfg(all(unix, not(target_os = "macos")))]
        let native = {
            use wry::WebViewExtUnix;
            view.view.webview()
        };
        if !crate::platform::imp::find(&native, &mut view.find, request.as_ref(), report) {
            eprintln!("engine: find is unavailable in this web engine");
        }
    }

    pub(crate) fn print(&self, id: ItemId) {
        if let Some(view) = self.views.get(&id) {
            let _ = view.print();
        }
    }

    pub(crate) fn open_devtools(&self, id: ItemId) {
        if let Some(view) = self.views.get(&id) {
            view.open_devtools();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autofill_reaches_only_a_tabs_own_view() {
        let source = include_str!("page_ops.rs");
        let body = source
            .split_once("pub(crate) fn autofill_password(")
            .unwrap()
            .1
            .split_once("\n    }\n")
            .unwrap()
            .0;
        assert!(body.contains("self.views.get(&id)"));
        for agent in ["agent", "work_resources", "Work"] {
            assert!(!body.contains(agent), "{agent}");
        }
    }

    #[test]
    fn zoom_settlement_keeps_the_last_proven_native_scale_on_failure() {
        assert_eq!(settled_zoom_scale(1.0, 1.25, true), 1.25);
        assert_eq!(settled_zoom_scale(1.25, 1.5, false), 1.25);
    }
}
