//! A tab's media for the sidebar's player. Nothing is polled and nothing is
//! installed in a page that does not play: the shell asks for a watch when a
//! tab starts making sound, and one host-initiated evaluation then waits in
//! the page for its media to change (play, pause, a seek, a new item, picture
//! in picture) and answers with what it is doing. The host asks again from
//! the answer until the document has no media left. Progress between answers
//! is the chrome's arithmetic, not the page's.

/// Finds the media element the player follows: in the document and up to
/// seven same-origin frames, the one playing and heard, else one paused part
/// way, else one finished; the longest wins a tie. Both scripts use it, so
/// they agree on the element without sharing any state.
const CHOOSE_JS: &str = r#"
  function documents() {
    var list = [document];
    for (var i = 0; i < list.length && list.length < 8; i++) {
      var frames = list[i].querySelectorAll('iframe,frame');
      for (var j = 0; j < frames.length && j < 16 && list.length < 8; j++) {
        try { if (frames[j].contentDocument) list.push(frames[j].contentDocument); } catch (_) {}
      }
    }
    return list;
  }
  function choose(docs) {
    var best = null, bestRank = -1, bestLength = -1;
    for (var i = 0; i < docs.length; i++) {
      var nodes = docs[i].querySelectorAll('video,audio');
      for (var j = 0; j < nodes.length && j < 64; j++) {
        var el = nodes[j];
        if (!el.currentSrc && !el.srcObject) continue;
        var playing = !el.paused && !el.ended;
        var resumable = el.paused && !el.ended && el.currentTime > 0;
        if (!playing && !resumable && !el.ended) continue;
        var heard = !el.muted && el.volume > 0;
        var rank = playing ? (heard ? 5 : 3) : resumable ? (heard ? 4 : 2) : heard ? 1 : 0;
        var length = isFinite(el.duration) ? el.duration : 0;
        if (rank > bestRank || (rank === bestRank && length > bestLength)) {
          best = el; bestRank = rank; bestLength = length;
        }
      }
    }
    return best;
  }
"#;

/// Waits for the page's media to change and answers with a JSON report
/// `{"k": token, "m": null | {"t", "a", "d", "p", "r", "s", "w", "e", "q",
/// "i", "n"}}` (see `zephium_core::media::parse_report`).
///
/// It answers at once when the state differs from `last` (the token of the
/// previous answer), and otherwise on the first media event that changes it,
/// or on any seek. Events are gathered for 300 ms before the page is read, so
/// a burst (a new item loading, a play that waits for data) is one answer.
/// Listeners, the title observer and the one timer are removed before every
/// answer, and a newer watch of the same document cancels this one.
pub(super) const MEDIA_WATCH_JS: &str = r#"(function(last){
  'use strict';
  var STOP = '__zephium_media_stop__';
  var GATHER_MS = 300;
  var EVENTS = ['play', 'playing', 'pause', 'waiting', 'ended', 'emptied', 'loadedmetadata',
      'durationchange', 'seeked', 'ratechange', 'enterpictureinpicture',
      'leavepictureinpicture', 'webkitpresentationmodechanged'];
  var win = window;
  __CHOOSE__
  function hash(text) {
    var value = 0x811c9dc5;
    for (var i = 0; i < text.length; i++) {
      value ^= text.charCodeAt(i);
      value = Math.imul(value, 0x01000193) >>> 0;
    }
    return value.toString(16);
  }
  function report(docs) {
    var el = choose(docs);
    if (!el) return {k: hash('none'), m: null};
    var meta = null;
    try { meta = navigator.mediaSession && navigator.mediaSession.metadata; } catch (_) {}
    var title = meta ? String(meta.title || '').slice(0, 512) : '';
    var artist = meta ? String(meta.artist || '').slice(0, 512) : '';
    var duration = isFinite(el.duration) && el.duration > 0 ? Number(el.duration) : null;
    var playing = !el.paused && !el.ended;
    var video = el.localName === 'video', owner = el.ownerDocument;
    var pip = video && (owner.pictureInPictureElement === el ||
        el.webkitPresentationMode === 'picture-in-picture');
    var canPip = video && !el.disablePictureInPicture && el.videoWidth > 0 &&
        ((owner.pictureInPictureEnabled && typeof el.requestPictureInPicture === 'function') ||
         (typeof el.webkitSupportsPresentationMode === 'function' &&
          el.webkitSupportsPresentationMode('picture-in-picture')));
    var item = hash(String(el.currentSrc || '').slice(0, 2048) + '\n' + title + '\n' + artist);
    var media = {
      t: title, a: artist, d: duration, p: Number(el.currentTime) || 0,
      r: Number(el.playbackRate) || 0, s: playing ? 1 : 0,
      w: playing && el.readyState < 3 ? 1 : 0, e: el.ended ? 1 : 0,
      q: canPip ? 1 : 0, i: pip ? 1 : 0, n: item
    };
    var state = [title, artist, duration === null ? null : Math.round(duration), media.r,
        media.s, media.w, media.e, media.q, media.i, item];
    return {k: hash(JSON.stringify(state)), m: media};
  }
  return new Promise(function(resolve) {
    var settled = false, timer = 0, seeked = false, observer = null, docs = [];
    function cleanup() {
      settled = true;
      for (var i = 0; i < docs.length; i++) {
        for (var j = 0; j < EVENTS.length; j++) {
          try { docs[i].removeEventListener(EVENTS[j], changed, true); } catch (_) {}
        }
      }
      try { if (observer) observer.disconnect(); } catch (_) {}
      if (timer) { try { win.clearTimeout(timer); } catch (_) {} }
      try { if (win[STOP] === stop) delete win[STOP]; } catch (_) {}
    }
    function stop() { if (!settled) { cleanup(); resolve(null); } }
    function answer(always) {
      if (settled) return;
      var now = report(documents());
      if (!always && now.k === last) return;
      cleanup();
      resolve(JSON.stringify(now));
    }
    function gathered() {
      timer = 0;
      var always = seeked;
      seeked = false;
      answer(always);
    }
    function changed(event) {
      if (settled) return;
      if (event && event.type === 'seeked') seeked = true;
      if (!timer) timer = win.setTimeout(gathered, GATHER_MS);
    }
    try {
      var earlier = win[STOP];
      if (typeof earlier === 'function') earlier();
      win[STOP] = stop;
      docs = documents();
      for (var i = 0; i < docs.length; i++) {
        for (var j = 0; j < EVENTS.length; j++) docs[i].addEventListener(EVENTS[j], changed, true);
      }
      var title = document.querySelector('title');
      if (title) {
        observer = new MutationObserver(function() { changed(null); });
        observer.observe(title, {childList: true, characterData: true, subtree: true});
      }
      answer(false);
    } catch (_) {
      if (!settled) { cleanup(); resolve(null); }
    }
  });
})(__LAST__)"#;

/// Acts on the element the watch follows. Run as the browser's own gesture,
/// in the content world automatic picture in picture uses, never the page's.
pub(super) const MEDIA_ACTION_JS: &str = r#"(async (action, value) => {
  __CHOOSE__
  try {
    const el = choose(documents());
    if (!el) return false;
    if (action === 'play') await el.play();
    else if (action === 'pause') el.pause();
    else if (action === 'seek') {
      if (!isFinite(el.duration)) return false;
      el.currentTime = Math.min(Math.max(0, value), el.duration);
    } else if (action === 'pip') {
      if (el.localName !== 'video') return false;
      const doc = el.ownerDocument;
      if (doc.pictureInPictureElement === el) await doc.exitPictureInPicture();
      else if (el.webkitPresentationMode === 'picture-in-picture') el.webkitSetPresentationMode('inline');
      else if (doc.pictureInPictureEnabled && typeof el.requestPictureInPicture === 'function')
        await el.requestPictureInPicture();
      else if (typeof el.webkitSetPresentationMode === 'function')
        el.webkitSetPresentationMode('picture-in-picture');
    }
    return true;
  } catch { return false; }
})"#;

pub(super) fn watch_expression(last: Option<&str>) -> String {
    let last = last
        .filter(|token| super::favicon::valid_token(token))
        .map_or_else(|| "null".to_owned(), |token| format!("\"{token}\""));
    MEDIA_WATCH_JS
        .replace("__CHOOSE__", CHOOSE_JS)
        .replace("__LAST__", &last)
}

/// The action for the document at `url` only; `None` for a seek that is not a
/// finite time.
pub(super) fn action_expression(
    url: &str,
    action: zephium_core::ports::engine::MediaAction,
) -> Option<String> {
    use zephium_core::ports::engine::MediaAction;
    let (name, value) = match action {
        MediaAction::Play => ("play", 0.0),
        MediaAction::Pause => ("pause", 0.0),
        MediaAction::Seek(seconds) if seconds.is_finite() && seconds >= 0.0 => ("seek", seconds),
        MediaAction::Seek(_) => return None,
        MediaAction::PictureInPicture => ("pip", 0.0),
    };
    let url = serde_json::to_string(url).ok()?;
    let script = MEDIA_ACTION_JS.replace("__CHOOSE__", CHOOSE_JS);
    Some(format!(
        "location.href === {url} ? ({script})('{name}', {value}) : false"
    ))
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod supported {
    use super::*;
    use crate::host::EngineHost;
    use crate::navigation_epoch::NavigationEpoch;
    use std::rc::Rc;
    use zephium_core::ids::ItemId;
    use zephium_core::media::{parse_report, MediaReport};
    use zephium_core::ports::engine::{EngineEvent, MediaAction};

    fn web(url: &str) -> bool {
        url.starts_with("https://") || url.starts_with("http://")
    }

    impl EngineHost {
        /// Starts watching the committed document of `id`, unless a watch is
        /// already running.
        pub(crate) fn watch_media(&mut self, id: ItemId) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            if view.media_watch.get().1 {
                return;
            }
            let Some((epoch, page_url)) = view.navigation.committed_snapshot() else {
                return;
            };
            if !web(&page_url) {
                return;
            }
            let generation = view.media_watch.get().0.wrapping_add(1).max(1);
            view.media_watch.set((generation, true));
            self.media_watch_run(id, generation, epoch, None);
        }

        fn media_watch_run(
            &self,
            id: ItemId,
            generation: u64,
            epoch: NavigationEpoch,
            last: Option<String>,
        ) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let started = crate::platform::cosmetic_pull::evaluate(
                &view.view,
                &watch_expression(last.as_deref()),
                move |answer| {
                    let report = answer.as_deref().and_then(parse_report);
                    let _ = crate::host::try_with(move |host| {
                        host.media_watch_report(id, generation, epoch, report);
                    });
                },
            );
            if !started {
                view.media_watch.set((generation, false));
            }
        }

        fn media_watch_report(
            &mut self,
            id: ItemId,
            generation: u64,
            epoch: NavigationEpoch,
            report: Option<MediaReport>,
        ) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            if view.media_watch.get() != (generation, true) {
                return;
            }
            if view.navigation.current_committed() != Some(epoch) {
                // The answer came from the document being left; the committed
                // one is asked afresh, and says itself whether it has media.
                view.media_watch.set((generation, false));
                self.watch_media(id);
                return;
            }
            let media = report
                .as_ref()
                .and_then(|report| report.media.clone())
                .map(Box::new);
            let more = media.is_some();
            view.event_permit
                .emit(&self.sink, EngineEvent::MediaChanged { id, media });
            match report.filter(|_| more) {
                Some(report) => self.media_watch_run(id, generation, epoch, Some(report.token)),
                None => view.media_watch.set((generation, false)),
            }
        }

        pub(crate) fn media_action(&self, id: ItemId, action: MediaAction) {
            let Some(view) = self.views.get(&id) else {
                return;
            };
            let Some((epoch, url)) = view.navigation.committed_snapshot() else {
                return;
            };
            if !web(&url) {
                return;
            }
            let Some(script) = action_expression(&url, action) else {
                return;
            };
            let permit = view.event_permit.clone();
            let navigation = view.navigation.clone();
            let current: Rc<dyn Fn() -> bool> = Rc::new(move || {
                permit.active_token().is_some() && navigation.current_committed() == Some(epoch)
            });
            crate::platform::imp::automatic_pip(&view.view, script, current);
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
impl super::EngineHost {
    pub(crate) fn watch_media(&mut self, _id: zephium_core::ids::ItemId) {}
    pub(crate) fn media_action(
        &self,
        _id: zephium_core::ids::ItemId,
        _action: zephium_core::ports::engine::MediaAction,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::ports::engine::MediaAction;

    #[test]
    fn the_watch_resumes_only_from_a_short_hex_token() {
        assert!(watch_expression(None).ends_with("})(null)"));
        assert!(watch_expression(Some("1a2b")).ends_with("})(\"1a2b\")"));
        for hostile in ["", "\");alert(1);(\"", "0123456789abcdef0", "g1"] {
            assert!(
                watch_expression(Some(hostile)).ends_with("})(null)"),
                "{hostile}"
            );
        }
        let expression = watch_expression(None);
        assert!(!expression.contains("__LAST__") && !expression.contains("__CHOOSE__"));
        assert!(expression.contains("function choose(docs)"));
    }

    #[test]
    fn the_watch_removes_what_it_installs_and_never_polls() {
        // Every path that resolves goes through `cleanup` first.
        assert_eq!(MEDIA_WATCH_JS.matches("resolve(").count(), 3);
        assert!(MEDIA_WATCH_JS.contains("removeEventListener(EVENTS[j], changed, true)"));
        assert!(MEDIA_WATCH_JS.contains("observer.disconnect()"));
        assert!(MEDIA_WATCH_JS.contains("clearTimeout(timer)"));
        // One pending timer at a time, armed only by an event.
        assert_eq!(MEDIA_WATCH_JS.matches("setTimeout").count(), 1);
        assert!(MEDIA_WATCH_JS.contains("if (!timer) timer = win.setTimeout"));
        for polling in [
            "setInterval",
            "requestAnimationFrame",
            "'timeupdate'",
            "'volumechange'",
        ] {
            assert!(!MEDIA_WATCH_JS.contains(polling), "{polling}");
            assert!(!CHOOSE_JS.contains(polling), "{polling}");
        }
    }

    #[test]
    fn an_action_names_its_document_and_carries_only_a_finite_time() {
        let url = "https://a.example/watch?v=1\"; alert(1); \"";
        let play = action_expression(url, MediaAction::Play).unwrap();
        assert!(play
            .starts_with(r#"location.href === "https://a.example/watch?v=1\"; alert(1); \"" ? "#));
        assert!(play.ends_with("('play', 0) : false"));
        assert!(action_expression(url, MediaAction::Seek(12.5))
            .unwrap()
            .ends_with("('seek', 12.5) : false"));
        assert!(action_expression(url, MediaAction::PictureInPicture)
            .unwrap()
            .ends_with("('pip', 0) : false"));
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(action_expression(url, MediaAction::Seek(bad)).is_none());
        }
    }
}
