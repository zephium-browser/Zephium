use zephium_core::ids::ItemId;
use zephium_core::navigation as navigation_policy;
use zephium_core::ports::engine::{EngineEvent, FindRequest, ZoomRequestId};

use super::scripts::{
    decode_favicon_eval_result, EXTRACT_HTML_JS, FAVICON_JS, MAX_HTML_CHARS, MAX_HTML_RESULT_BYTES,
};
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
            view.applied_zoom = settled_zoom_scale(view.applied_zoom, scale, succeeded);
            (view.event_permit.clone(), view.applied_zoom, succeeded)
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

    pub(crate) fn extract_html(&self, id: ItemId) {
        if let Some(view) = self.views.get(&id) {
            let Some(epoch) = view.navigation.current_committed() else {
                return;
            };
            let sink = self.sink.clone();
            let permit = view.event_permit.clone();
            let navigation = view.navigation.clone();
            let script = EXTRACT_HTML_JS.replace("__MAX__", &MAX_HTML_CHARS.to_string());
            let _ = view.evaluate_script_with_callback(&script, move |result| {
                if !navigation.is_current(epoch) {
                    return;
                }
                if result.len() > MAX_HTML_RESULT_BYTES {
                    return;
                }
                let Ok(value) = serde_json::from_str::<String>(&result) else {
                    return;
                };
                let (truncated, html) = if let Some(html) = value.strip_prefix('0') {
                    (false, html)
                } else if let Some(html) = value.strip_prefix('1') {
                    (true, html)
                } else {
                    return;
                };
                if html.encode_utf16().count() > MAX_HTML_CHARS {
                    return;
                }
                permit.emit(
                    &sink,
                    EngineEvent::HtmlExtracted {
                        id,
                        html: html.to_owned(),
                        truncated,
                    },
                );
            });
        }
    }

    pub(crate) fn discover_favicon(&self, id: ItemId) {
        if let Some(view) = self.views.get(&id) {
            let Some((epoch, page_url)) = view.navigation.committed_snapshot() else {
                return;
            };
            if !navigation_policy::is_allowed_str(&page_url) {
                return;
            }
            let sink = self.sink.clone();
            let permit = view.event_permit.clone();
            let navigation = view.navigation.clone();
            let _ = view.evaluate_script_with_callback(FAVICON_JS, move |result| {
                if !navigation.matches_committed_snapshot(epoch, &page_url) {
                    return;
                }
                // Native engines JSON-serialize the primitive callback value.
                // The bounded decoder accepts both ordinary JSON and
                // Foundation's escaped-solidus spelling, then requires one
                // canonical 5464-byte base64 value and exact 32x32 RGBA
                // output. No page object/toJSON hook is traversed.
                let Some(rgba) = decode_favicon_eval_result(&result) else {
                    return;
                };
                permit.emit(
                    &sink,
                    EngineEvent::FaviconPixels {
                        id,
                        page_url: page_url.clone(),
                        rgba,
                    },
                );
            });
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
    fn zoom_settlement_keeps_the_last_proven_native_scale_on_failure() {
        assert_eq!(settled_zoom_scale(1.0, 1.25, true), 1.25);
        assert_eq!(settled_zoom_scale(1.25, 1.5, false), 1.25);
    }
}
