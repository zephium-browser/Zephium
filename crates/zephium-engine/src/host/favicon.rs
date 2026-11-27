//! Icon discovery for a tab's committed document. Nothing is polled: one
//! host-initiated evaluation waits inside the page for its icon links, as
//! the head fills and as they change until shortly after load, and answers
//! with the addresses they name. Fetching and decoding happen elsewhere; only
//! a vector or cookie-gated icon is ever decoded by the page, from one
//! address the host names, and only a fixed 32x32 raster comes back.

/// Waits for the document's icon links and answers with a JSON report
/// `{"u": address, "d": 0|1, "k": token, "c": [[rel, href, sizes, type]]}`.
///
/// It answers at the first moment the links differ from `last` (the token of
/// the previous report), so a head that fills after commit is reported as soon
/// as it has icons, and again if they change. A loaded document is reported
/// even with none, then watched a short while longer before the final report
/// (`d` 1).
/// A page that never loads is answered after thirty seconds, or cancelled by
/// its navigation. Observers, listeners and the timer are removed before every
/// answer, and a newer watch of the same document cancels this one.
pub(super) const FAVICON_WATCH_JS: &str = r#"(function(last){
  'use strict';
  var STOP = '__zephium_favicon_stop__';
  var SELECTOR = 'link[rel~="icon"],link[rel~="apple-touch-icon"],' +
      'link[rel~="apple-touch-icon-precomposed"],link[rel~="mask-icon"]';
  var WINDOW_MS = 1500, HARD_MS = 30000;
  var doc = document, win = window;
  return new Promise(function(resolve) {
    var settled = false, observer = null, observing = null, timer = 0;
    function cleanup() {
      settled = true;
      try { if (observer) observer.disconnect(); } catch (_) {}
      try { doc.removeEventListener('DOMContentLoaded', check, true); } catch (_) {}
      try { doc.removeEventListener('readystatechange', check, true); } catch (_) {}
      try { win.removeEventListener('load', onload, true); } catch (_) {}
      if (timer) { try { win.clearTimeout(timer); } catch (_) {} }
      try { if (win[STOP] === stop) delete win[STOP]; } catch (_) {}
    }
    function stop() { if (!settled) { cleanup(); resolve(null); } }
    function snapshot() {
      var nodes = doc.querySelectorAll(SELECTOR), list = [];
      for (var i = 0; i < nodes.length && i < 64 && list.length < 32; i++) {
        var link = nodes[i], href = link.getAttribute('href');
        if (!href || href.length > 2048) continue;
        var rel = (link.getAttribute('rel') || '').toLowerCase().split(/\s+/);
        var kind = rel.indexOf('icon') >= 0 ? 'i' :
            (rel.indexOf('apple-touch-icon') >= 0 ||
             rel.indexOf('apple-touch-icon-precomposed') >= 0) ? 't' :
            rel.indexOf('mask-icon') >= 0 ? 'm' : '';
        if (!kind) continue;
        var absolute;
        try { absolute = new URL(href, doc.baseURI).href; } catch (_) { continue; }
        if (absolute.length > 2048) continue;
        list.push([kind, absolute, String(link.getAttribute('sizes') || '').slice(0, 64),
            String(link.getAttribute('type') || '').slice(0, 64)]);
      }
      return list;
    }
    function token(list) {
      var text = JSON.stringify(list), hash = 0x811c9dc5;
      for (var i = 0; i < text.length; i++) {
        hash ^= text.charCodeAt(i);
        hash = Math.imul(hash, 0x01000193) >>> 0;
      }
      return hash.toString(16);
    }
    function finish(done, list) {
      if (settled) return;
      cleanup();
      resolve(JSON.stringify({
        u: String(doc.location.href).slice(0, 8192), d: done ? 1 : 0, k: token(list), c: list
      }));
    }
    function scan(loaded) {
      if (settled) return;
      var list = snapshot();
      if ((list.length || loaded) && token(list) !== last) finish(false, list);
    }
    function check() { scan(false); }
    function finalize(ms) {
      if (timer) win.clearTimeout(timer);
      timer = win.setTimeout(function() { finish(true, snapshot()); }, ms);
    }
    function onload() {
      scan(true);
      if (!settled) finalize(WINDOW_MS);
    }
    function relevant(node) { return node && node.nodeType === 1 && node.localName === 'link'; }
    function touches(record) {
      if (record.type === 'attributes') return relevant(record.target);
      var i;
      for (i = 0; i < record.addedNodes.length; i++) if (relevant(record.addedNodes[i])) return true;
      for (i = 0; i < record.removedNodes.length; i++) if (relevant(record.removedNodes[i])) return true;
      return false;
    }
    function onmutation(records) {
      if (settled) return;
      var moved = doc.head && observing !== doc.head;
      if (moved) observe();
      for (var i = 0; i < records.length; i++) {
        if (moved || touches(records[i])) { check(); return; }
      }
    }
    function observe() {
      var target = doc.head || doc.documentElement || doc;
      if (observing === target) return;
      try { if (observer) observer.disconnect(); } catch (_) {}
      observer = new MutationObserver(onmutation);
      if (target === doc.head) {
        observer.observe(target, {childList: true, subtree: true, attributes: true,
            attributeFilter: ['rel', 'href', 'sizes', 'type']});
      } else {
        observer.observe(target, {childList: true});
      }
      observing = target;
    }
    try {
      var earlier = win[STOP];
      if (typeof earlier === 'function') earlier();
      win[STOP] = stop;
      doc.addEventListener('DOMContentLoaded', check, true);
      doc.addEventListener('readystatechange', check, true);
      win.addEventListener('load', onload, true);
      observe();
      check();
      if (!settled) finalize(HARD_MS);
      if (!settled && doc.readyState === 'complete') onload();
    } catch (_) {
      if (!settled) { cleanup(); resolve(null); }
    }
  });
})(__LAST__)"#;

/// Loads one icon address in the page and answers its canonical 32x32 RGBA
/// base64, or null. Same-origin addresses carry the page's cookies, as an
/// image tag would; any other asks as an anonymous cross-origin image, which
/// the canvas can read only when the server allows it.
pub(super) const FAVICON_RASTER_JS: &str = r#"(function(source){
  'use strict';
  return new Promise(function(resolve) {
    var finished = false;
    function end(value) { if (!finished) { finished = true; resolve(value); } }
    try {
      var url = new URL(source, document.baseURI);
      if ((url.protocol !== 'http:' && url.protocol !== 'https:') || url.href.length > 2048) {
        end(null);
        return;
      }
      var image = new Image();
      image.decoding = 'async';
      if (url.origin !== document.location.origin) image.crossOrigin = 'anonymous';
      image.onerror = function() { end(null); };
      image.onload = function() {
        try {
          var width = image.naturalWidth, height = image.naturalHeight;
          if (!Number.isFinite(width) || !Number.isFinite(height) ||
              width < 1 || height < 1 || width > 16384 || height > 16384) {
            end(null);
            return;
          }
          var canvas = document.createElement('canvas');
          canvas.width = 32;
          canvas.height = 32;
          var context = canvas.getContext('2d', {alpha: true, willReadFrequently: true});
          if (!context) { end(null); return; }
          context.clearRect(0, 0, 32, 32);
          var scale = Math.min(32 / width, 32 / height);
          var drawWidth = Math.max(1, Math.round(width * scale));
          var drawHeight = Math.max(1, Math.round(height * scale));
          context.drawImage(image, Math.floor((32 - drawWidth) / 2),
              Math.floor((32 - drawHeight) / 2), drawWidth, drawHeight);
          var bytes = context.getImageData(0, 0, 32, 32).data, binary = '';
          for (var i = 0; i < bytes.length; i++) binary += String.fromCharCode(bytes[i]);
          end(btoa(binary));
        } catch (_) {
          end(null);
        }
      };
      image.src = url.href;
    } catch (_) {
      end(null);
    }
  });
})(__URL__)"#;

/// A token the page produced, echoed back so a watch resumes from it. Only
/// short hex is ever placed in a script.
pub(super) fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 16 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn watch_expression(last: Option<&str>) -> String {
    let last = last
        .filter(|token| valid_token(token))
        .map_or_else(|| "null".to_owned(), |token| format!("\"{token}\""));
    FAVICON_WATCH_JS.replace("__LAST__", &last)
}

pub(super) fn raster_expression(url: &str) -> Option<String> {
    if url.len() > zephium_core::icon::MAX_CANDIDATE_URL_BYTES {
        return None;
    }
    let literal = serde_json::to_string(url).ok()?;
    Some(FAVICON_RASTER_JS.replace("__URL__", &literal))
}

/// What the page's decoded answer means for a watch of `page_url`.
pub(super) fn same_address(reported: &str, page_url: &str) -> bool {
    let (Ok(mut reported), Ok(mut page)) = (url::Url::parse(reported), url::Url::parse(page_url))
    else {
        return false;
    };
    reported.set_fragment(None);
    page.set_fragment(None);
    reported == page
}

/// Reports one watch may make for a document: the head's first icons, a few
/// changes, and the last. A page rewriting its icon links forever cannot keep
/// the watch, or the host, busy.
pub(super) const MAX_WATCH_RUNS: u8 = 6;

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod supported {
    use super::*;
    use crate::host::EngineHost;
    use zephium_core::icon::{parse_report, CandidateReport};
    use zephium_core::ids::ItemId;
    use zephium_core::navigation as navigation_policy;
    use zephium_core::ports::engine::EngineEvent;

    /// One watch of `view`'s document. `done` runs on the native callback
    /// thread with the page's report, or `None` when it was cancelled, the
    /// document left, or the answer was not a valid report.
    pub(crate) fn watch(
        view: &wry::WebView,
        last: Option<&str>,
        done: impl FnOnce(Option<CandidateReport>) + Send + 'static,
    ) -> bool {
        crate::platform::cosmetic_pull::evaluate(view, &watch_expression(last), move |result| {
            done(result.as_deref().and_then(parse_report))
        })
    }

    /// One icon address decoded by the page into the fixed raster, if it
    /// would.
    pub(crate) fn rasterize(
        view: &wry::WebView,
        url: &str,
        done: impl FnOnce(Option<Vec<u8>>) + Send + 'static,
    ) -> bool {
        let Some(expression) = raster_expression(url) else {
            return false;
        };
        crate::platform::cosmetic_pull::evaluate(view, &expression, move |result| {
            done(
                result
                    .as_deref()
                    .and_then(zephium_core::icon::decode_rgba32),
            )
        })
    }

    impl EngineHost {
        /// Starts watching the committed document of `id`. The newest call is
        /// the only watch whose reports are used.
        pub(crate) fn discover_favicon(&mut self, id: ItemId) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let Some((epoch, page_url)) = view.navigation.committed_snapshot() else {
                return;
            };
            if !navigation_policy::is_allowed_str(&page_url) {
                return;
            }
            let generation = view.favicon_watch.get().wrapping_add(1).max(1);
            view.favicon_watch.set(generation);
            self.favicon_watch_run(id, generation, epoch, page_url, None, 1);
        }

        fn favicon_watch_run(
            &self,
            id: ItemId,
            generation: u64,
            epoch: crate::navigation_epoch::NavigationEpoch,
            page_url: String,
            last: Option<String>,
            run: u8,
        ) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let resumed = last.clone();
            let started = watch(&view.view, last.as_deref(), move |report| {
                let _ = crate::host::try_with(move |host| {
                    host.favicon_watch_report(
                        id, generation, epoch, page_url, resumed, run, report,
                    );
                });
            });
            if !started {
                eprintln!("engine: favicon watch was not admitted");
            }
        }

        #[allow(clippy::too_many_arguments)]
        fn favicon_watch_report(
            &mut self,
            id: ItemId,
            generation: u64,
            epoch: crate::navigation_epoch::NavigationEpoch,
            page_url: String,
            last: Option<String>,
            run: u8,
            report: Option<CandidateReport>,
        ) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            if view.favicon_watch.get() != generation
                || !view.navigation.matches_committed_snapshot(epoch, &page_url)
            {
                return;
            }
            let Some(report) = report else {
                return;
            };
            if !same_address(&report.page_url, &page_url) {
                // The script ran in the document being left; the committed one
                // has no watch yet.
                if run < MAX_WATCH_RUNS {
                    self.favicon_watch_run(id, generation, epoch, page_url, last, run + 1);
                }
                return;
            }
            let complete = report.complete || run >= MAX_WATCH_RUNS;
            view.event_permit.emit(
                &self.sink,
                EngineEvent::FaviconCandidates {
                    id,
                    page_url: page_url.clone(),
                    complete,
                    candidates: report.candidates,
                },
            );
            if !complete {
                self.favicon_watch_run(
                    id,
                    generation,
                    epoch,
                    page_url,
                    Some(report.token),
                    run + 1,
                );
            }
        }

        /// Decodes one declared icon address in the page of `id`.
        pub(crate) fn rasterize_favicon(&mut self, id: ItemId, url: String) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let Some((epoch, page_url)) = view.navigation.committed_snapshot() else {
                return;
            };
            if !navigation_policy::is_allowed_str(&page_url) {
                view.event_permit.emit(
                    &self.sink,
                    EngineEvent::FaviconRasterFailed { id, page_url, url },
                );
                return;
            }
            let queued_url = url.clone();
            let queued_page = page_url.clone();
            let started = rasterize(&view.view, &url, move |rgba| {
                let _ = crate::host::try_with(move |host| {
                    host.favicon_raster_done(id, epoch, queued_page, queued_url, rgba);
                });
            });
            if !started {
                view.event_permit.emit(
                    &self.sink,
                    EngineEvent::FaviconRasterFailed { id, page_url, url },
                );
            }
        }

        fn favicon_raster_done(
            &mut self,
            id: ItemId,
            epoch: crate::navigation_epoch::NavigationEpoch,
            page_url: String,
            url: String,
            rgba: Option<Vec<u8>>,
        ) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            if !view.navigation.matches_committed_snapshot(epoch, &page_url) {
                return;
            }
            let event = match rgba {
                Some(rgba) => EngineEvent::FaviconPixels { id, page_url, rgba },
                None => EngineEvent::FaviconRasterFailed { id, page_url, url },
            };
            view.event_permit.emit(&self.sink, event);
        }
    }
}

#[cfg(all(feature = "agentic-browser", target_os = "macos"))]
pub(super) use supported::{rasterize, watch};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl super::EngineHost {
    pub(crate) fn discover_favicon(&mut self, _id: zephium_core::ids::ItemId) {}
    pub(crate) fn rasterize_favicon(&mut self, _id: zephium_core::ids::ItemId, _url: String) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watch_resumes_only_from_a_short_hex_token() {
        assert!(watch_expression(None).ends_with("})(null)"));
        assert!(watch_expression(Some("1a2b3c4d")).ends_with("})(\"1a2b3c4d\")"));
        for hostile in ["", "x", "\");alert(1);(\"", "0123456789abcdef0", "g1"] {
            assert!(
                watch_expression(Some(hostile)).ends_with("})(null)"),
                "{hostile}"
            );
        }
        assert!(!watch_expression(None).contains("__LAST__"));
    }

    #[test]
    fn a_raster_address_is_embedded_as_one_json_string() {
        let expression = raster_expression("https://a.example/i.png\"); alert(1); (\"").unwrap();
        assert!(expression.ends_with(r#"})("https://a.example/i.png\"); alert(1); (\"")"#));
        assert!(!expression.contains("__URL__"));
        let long = format!(
            "https://a.example/{}",
            "a".repeat(zephium_core::icon::MAX_CANDIDATE_URL_BYTES)
        );
        assert!(raster_expression(&long).is_none());
    }

    #[test]
    fn the_watch_script_removes_what_it_installs_before_answering() {
        // Every path that resolves goes through `cleanup` first.
        assert_eq!(FAVICON_WATCH_JS.matches("resolve(").count(), 3);
        assert!(FAVICON_WATCH_JS.contains("observer.disconnect()"));
        assert!(FAVICON_WATCH_JS.contains("removeEventListener('load'"));
        assert!(FAVICON_WATCH_JS.contains("clearTimeout"));
        assert!(FAVICON_WATCH_JS.contains("attributeFilter: ['rel', 'href', 'sizes', 'type']"));
        // One pending timer at a time; no interval.
        assert_eq!(FAVICON_WATCH_JS.matches("setTimeout").count(), 1);
        assert!(FAVICON_WATCH_JS.contains("clearTimeout(timer)"));
        assert!(!FAVICON_WATCH_JS.contains("setInterval"));
        assert!(!FAVICON_WATCH_JS.contains("requestAnimationFrame"));
    }

    #[test]
    fn a_report_names_the_document_it_was_made_for_up_to_its_fragment() {
        assert!(same_address(
            "https://a.example/x?y=1#one",
            "https://a.example/x?y=1#two"
        ));
        assert!(!same_address("https://a.example/x", "https://a.example/y"));
        assert!(!same_address("https://a.example/", "https://b.example/"));
        assert!(!same_address("garbage", "https://a.example/"));
    }
}
