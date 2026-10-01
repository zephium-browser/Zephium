use std::collections::HashMap;

use zephium_core::ids::ScriptId;
use zephium_core::injection::MatchSet;
use zephium_core::ports::engine::{
    ContentScope, EngineEvent, Partition, RunAt, ScriptOwner, Shortcut, UserContent,
    UserContentApplyFailure, UserContentGeneration, UserContentSettlement, UserScript,
    UserScriptRefusal, UserScriptRefusalReason, UserStyle, World,
};

use super::EngineHost;

// Installed before every page script. It records otherwise non-enumerable
// unload/audio/capture state, while the query itself directly compares form
// controls with their defaults. This is a data-loss guard, not a capability:
// it exposes no Rust/native object and returns only a fixed boolean schema.
// Any hook replacement, excessive state, child frame, or exception becomes
// `uncertain`, which vetoes discard.
pub(super) const DISCARD_SAFETY_BOOTSTRAP_JS: &str = r#"(function(){
  'use strict';
  var key = '__zephium_discard_safety_v1__';
  if (Object.prototype.hasOwnProperty.call(globalThis, key)) return;
  var uncertain = false;
  var beforeUnload = [];
  var audioContexts = new Set();
  var captureTracks = new Set();
  var MAX_TRACKED = 4096;
  var MAX_SHADOW_ROOTS = 256;
  var shadowRoots = [];
  var shadowRootSet = new WeakSet();
  var apply = Reflect.apply;
  var construct = Reflect.construct;
  var weakSetAdd = WeakSet.prototype.add;
  var weakSetHas = WeakSet.prototype.has;
  var arrayPush = Array.prototype.push;

  function captureGetter(proto, name) {
    try {
      var descriptor = Object.getOwnPropertyDescriptor(proto, name);
      return descriptor && typeof descriptor.get === 'function' ? descriptor.get : null;
    } catch (_) { uncertain = true; return null; }
  }
  function callGetter(getter, value) {
    if (!getter) throw new Error('missing platform getter');
    return apply(getter, value, []);
  }

  var createTreeWalker = Document.prototype.createTreeWalker;
  var walkerNext = TreeWalker.prototype.nextNode;
  var elementMatches = Element.prototype.matches;
  var WeakReference = globalThis.WeakRef;
  var weakDeref = WeakReference && WeakReference.prototype.deref;
  var readyState = captureGetter(Document.prototype, 'readyState');
  var shadowRootGetter = captureGetter(Element.prototype, 'shadowRoot');
  var inputType = captureGetter(HTMLInputElement.prototype, 'type');
  var inputValue = captureGetter(HTMLInputElement.prototype, 'value');
  var inputDefaultValue = captureGetter(HTMLInputElement.prototype, 'defaultValue');
  var inputChecked = captureGetter(HTMLInputElement.prototype, 'checked');
  var inputDefaultChecked = captureGetter(HTMLInputElement.prototype, 'defaultChecked');
  var inputFiles = captureGetter(HTMLInputElement.prototype, 'files');
  var textareaValue = captureGetter(HTMLTextAreaElement.prototype, 'value');
  var textareaDefaultValue = captureGetter(HTMLTextAreaElement.prototype, 'defaultValue');
  var selectOptions = captureGetter(HTMLSelectElement.prototype, 'options');
  var optionSelected = captureGetter(HTMLOptionElement.prototype, 'selected');
  var optionDefaultSelected = captureGetter(HTMLOptionElement.prototype, 'defaultSelected');
  var mediaPaused = captureGetter(HTMLMediaElement.prototype, 'paused');
  var mediaEnded = captureGetter(HTMLMediaElement.prototype, 'ended');
  var mediaReadyState = captureGetter(HTMLMediaElement.prototype, 'readyState');
  var mediaSrcObject = captureGetter(HTMLMediaElement.prototype, 'srcObject');
  var trackReadyState = globalThis.MediaStreamTrack &&
      captureGetter(MediaStreamTrack.prototype, 'readyState');
  var getTracks = globalThis.MediaStream && MediaStream.prototype.getTracks;
  var contextState = globalThis.BaseAudioContext &&
      captureGetter(BaseAudioContext.prototype, 'state');

  var elementProto = globalThis.Element && Element.prototype;
  var originalAttachShadow = elementProto && elementProto.attachShadow;
  var wrappedAttachShadow = originalAttachShadow;
  function rememberShadowRoot(root) {
    if (!root || apply(weakSetHas, shadowRootSet, [root])) return;
    if (!WeakReference || !weakDeref) { uncertain = true; return; }
    if (shadowRoots.length >= MAX_SHADOW_ROOTS) {
      // The observer must not keep detached component trees alive. Compact
      // only when admission needs space; no polling or finalizer is required.
      var retained = 0;
      for (var i = 0; i < shadowRoots.length; i++) {
        if (apply(weakDeref, shadowRoots[i], [])) shadowRoots[retained++] = shadowRoots[i];
      }
      shadowRoots.length = retained;
    }
    if (shadowRoots.length >= MAX_SHADOW_ROOTS) {
      uncertain = true;
      return;
    }
    apply(weakSetAdd, shadowRootSet, [root]);
    apply(arrayPush, shadowRoots, [new WeakReference(root)]);
  }
  try {
    if (typeof originalAttachShadow !== 'function' || !createTreeWalker || !walkerNext || !elementMatches || !weakDeref || !shadowRootGetter) {
      uncertain = true;
    } else {
      wrappedAttachShadow = function() {
        var root = apply(originalAttachShadow, this, arguments);
        rememberShadowRoot(root);
        return root;
      };
      elementProto.attachShadow = wrappedAttachShadow;
    }
  } catch (_) { uncertain = true; }

  var eventTarget = globalThis.EventTarget && EventTarget.prototype;
  var originalAdd = eventTarget && eventTarget.addEventListener;
  var originalRemove = eventTarget && eventTarget.removeEventListener;
  var wrappedAdd = originalAdd;
  var wrappedRemove = originalRemove;
  function captureOption(options) {
    return options === true || (!!options && typeof options === 'object' && options.capture === true);
  }
  try {
    wrappedAdd = function(type, listener, options) {
      if ((this === undefined || this === globalThis) && String(type).toLowerCase() === 'beforeunload' && listener != null) {
        var capture = captureOption(options);
        var found = false;
        for (var i = 0; i < beforeUnload.length; i++) {
          if (beforeUnload[i][0] === listener && beforeUnload[i][1] === capture) { found = true; break; }
        }
        if (!found) {
          if (beforeUnload.length < MAX_TRACKED) beforeUnload.push([listener, capture]);
          else uncertain = true;
        }
      }
      return apply(originalAdd, this, arguments);
    };
    wrappedRemove = function(type, listener, options) {
      var result = apply(originalRemove, this, arguments);
      if ((this === undefined || this === globalThis) && String(type).toLowerCase() === 'beforeunload' && listener != null) {
        var capture = captureOption(options);
        for (var i = 0; i < beforeUnload.length; i++) {
          if (beforeUnload[i][0] === listener && beforeUnload[i][1] === capture) {
            beforeUnload.splice(i, 1); break;
          }
        }
      }
      return result;
    };
    eventTarget.addEventListener = wrappedAdd;
    eventTarget.removeEventListener = wrappedRemove;
  } catch (_) { uncertain = true; }

  function observeStream(stream) {
    try {
      var tracks = apply(getTracks, stream, []);
      for (var i = 0; i < tracks.length; i++) {
        if (captureTracks.size < MAX_TRACKED) captureTracks.add(tracks[i]);
        else uncertain = true;
      }
    } catch (_) { uncertain = true; }
    return stream;
  }
  function wrapMediaMethod(proto, name) {
    try {
      if (!proto || typeof proto[name] !== 'function') return;
      var original = proto[name];
      var wrapped = function() {
        var result = apply(original, this, arguments);
        return Promise.resolve(result).then(observeStream);
      };
      proto[name] = wrapped;
      return [proto, name, wrapped];
    } catch (_) { uncertain = true; return null; }
  }
  var mediaHooks = [];
  if (globalThis.MediaDevices) {
    mediaHooks.push(wrapMediaMethod(MediaDevices.prototype, 'getUserMedia'));
    mediaHooks.push(wrapMediaMethod(MediaDevices.prototype, 'getDisplayMedia'));
  }

  var audioHooks = [];
  function wrapAudioConstructor(name) {
    try {
      var original = globalThis[name];
      if (typeof original !== 'function') return;
      var wrapped = new Proxy(original, {
        construct: function(target, args, newTarget) {
          var value = construct(target, args, newTarget === wrapped ? target : newTarget);
          if (audioContexts.size < MAX_TRACKED) audioContexts.add(value);
          else uncertain = true;
          return value;
        }
      });
      globalThis[name] = wrapped;
      audioHooks.push([name, wrapped]);
    } catch (_) { uncertain = true; }
  }
  wrapAudioConstructor('AudioContext');
  if (globalThis.webkitAudioContext !== globalThis.AudioContext) {
    wrapAudioConstructor('webkitAudioContext');
  }

  function liveTrack(track) {
    return callGetter(trackReadyState, track) === 'live';
  }
  function report() {
    var dirtyForm = false;
    var editable = false;
    var media = false;
    var audioContext = false;
    var capture = false;
    var childFrames = false;
    var localUncertain = uncertain;
    try {
      if (eventTarget.addEventListener !== wrappedAdd || eventTarget.removeEventListener !== wrappedRemove) {
        localUncertain = true;
      }
      for (var h = 0; h < mediaHooks.length; h++) {
        var hook = mediaHooks[h];
        if (hook && hook[0][hook[1]] !== hook[2]) localUncertain = true;
      }
      for (var a = 0; a < audioHooks.length; a++) {
        if (globalThis[audioHooks[a][0]] !== audioHooks[a][1]) localUncertain = true;
      }
      if (!elementProto || elementProto.attachShadow !== wrappedAttachShadow) {
        localUncertain = true;
      }

      // One bounded snapshot replaces full-document querySelectorAll arrays
      // and repeated shadow-tree scans. The former '*' query allocated every
      // element before enforcing MAX_TRACKED. Oversized/uncertain documents
      // remain protected; a partial scan can never authorize discard.
      if (!createTreeWalker || !walkerNext || !elementMatches || !weakDeref) return 256;
      var snapshotElements = [];
      function append(scope) {
        var walker = apply(createTreeWalker, document, [scope, 1]);
        var node;
        while ((node = apply(walkerNext, walker, []))) {
          if (snapshotElements.length >= MAX_TRACKED) return false;
          apply(arrayPush, snapshotElements, [node]);
          // Parser-created open roots may bypass attachShadow. Inspect them
          // too, but keep the existing uncertainty veto for their discovery.
          var untracked = callGetter(shadowRootGetter, node);
          if (untracked && !apply(weakSetHas, shadowRootSet, [untracked])) {
            localUncertain = true;
            rememberShadowRoot(untracked);
          }
        }
        return true;
      }
      if (!append(document)) return 256;
      for (var r = 0; r < shadowRoots.length; r++) {
        var root = apply(weakDeref, shadowRoots[r], []);
        if (root && !append(root)) return 256;
      }
      // A live root beyond the tracking cap is always a veto, including one
      // discovered while walking the current snapshot.
      localUncertain = localUncertain || uncertain;
      function collect(selector) {
        var values = [];
        for (var n = 0; n < snapshotElements.length; n++) {
          if (apply(elementMatches, snapshotElements[n], [selector])) apply(arrayPush, values, [snapshotElements[n]]);
        }
        return values;
      }
      if (collect('template[shadowrootmode],template[shadowroot]').length !== 0) {
        localUncertain = true;
      }

      var controls = collect('input,textarea,select');
      if (controls.length > MAX_TRACKED) localUncertain = true;
      for (var i = 0; i < controls.length && i < MAX_TRACKED && !dirtyForm; i++) {
        var control = controls[i];
        if (control instanceof HTMLInputElement) {
          var type = String(callGetter(inputType, control)).toLowerCase();
          if (type === 'checkbox' || type === 'radio') {
            dirtyForm = callGetter(inputChecked, control) !== callGetter(inputDefaultChecked, control);
          } else if (type === 'file') {
            var files = callGetter(inputFiles, control);
            dirtyForm = !!files && files.length !== 0;
          } else if (type !== 'button' && type !== 'submit' && type !== 'reset' &&
                     type !== 'image' && type !== 'hidden') {
            dirtyForm = callGetter(inputValue, control) !== callGetter(inputDefaultValue, control);
          }
        } else if (control instanceof HTMLTextAreaElement) {
          dirtyForm = callGetter(textareaValue, control) !== callGetter(textareaDefaultValue, control);
        } else if (control instanceof HTMLSelectElement) {
          var options = callGetter(selectOptions, control);
          if (options.length > MAX_TRACKED) localUncertain = true;
          for (var o = 0; o < options.length && o < MAX_TRACKED; o++) {
            if (callGetter(optionSelected, options[o]) !== callGetter(optionDefaultSelected, options[o])) {
              dirtyForm = true; break;
            }
          }
        }
      }

      editable = document.designMode === 'on' ||
          collect('[contenteditable]:not([contenteditable="false" i])').length !== 0;
      var elements = collect('audio,video');
      if (elements.length > MAX_TRACKED) localUncertain = true;
      for (var m = 0; m < elements.length && m < MAX_TRACKED; m++) {
        var element = elements[m];
        if (!callGetter(mediaPaused, element) && !callGetter(mediaEnded, element) &&
            callGetter(mediaReadyState, element) > 0) media = true;
        var stream = callGetter(mediaSrcObject, element);
        if (stream && getTracks) {
          var tracks = apply(getTracks, stream, []);
          for (var t = 0; t < tracks.length; t++) if (liveTrack(tracks[t])) capture = true;
        }
      }
      audioContexts.forEach(function(context) {
        if (callGetter(contextState, context) === 'running') audioContext = true;
      });
      captureTracks.forEach(function(track) { if (liveTrack(track)) capture = true; });
      childFrames = collect('iframe,frame').length !== 0;
    } catch (_) { localUncertain = true; }

    // Fixed primitive bitmask: ready=bit0; every protection/uncertainty fact
    // occupies bits1..8. Native JSON conversion can therefore produce at
    // most three ASCII bytes and never traverses a page-controlled toJSON.
    return (callGetter(readyState, document) === 'complete' ? 1 : 0) |
      ((beforeUnload.length !== 0 || typeof globalThis.onbeforeunload === 'function') ? 2 : 0) |
      (dirtyForm ? 4 : 0) | (editable ? 8 : 0) | (media ? 16 : 0) |
      (audioContext ? 32 : 0) | (capture ? 64 : 0) |
      (childFrames ? 128 : 0) | (localUncertain ? 256 : 0);
  }
  try {
    Object.defineProperty(globalThis, key, {
      value: report, writable: false, configurable: false, enumerable: false
    });
  } catch (_) { uncertain = true; }
})()"#;

pub(super) const DISCARD_SAFETY_QUERY_JS: &str = r#"(function(){
  'use strict';
  try {
    var query = globalThis.__zephium_discard_safety_v1__;
    if (typeof query !== 'function') throw new Error('missing safety tracker');
    return query();
  } catch (_) {
    return 256;
  }
})()"#;

// Fetch and decode entirely inside the untrusted site renderer. The callback
// surface is a fixed 32x32 RGBA raster; privileged Rust/chrome never parse a
// page-controlled image container. Calling this script again polls the
// renderer-owned asynchronous Image decode without adding an IPC bridge.
pub(super) const FAVICON_JS: &str = r#"(function(){
  'use strict';
  var pageUrl = String(document.location.href).slice(0, 8192);
  var key = '__zephium_favicon_rgba32_v1__';
  var readyState = document.readyState === 'complete' ? 'complete' :
      (document.readyState === 'interactive' ? 'interactive' : 'loading');
  var state = globalThis[key];
  var rebuildAfterComplete = false;
  function validRgba(value) {
    // 32 * 32 * 4 bytes encode to exactly 5464 canonical base64
    // characters, with two padding characters. Check length before the
    // regular expression so a page-preseeded huge string is never copied
    // into the renderer-to-host callback result.
    return typeof value === 'string' && value.length === 5464 &&
        /^[A-Za-z0-9+/]{5462}==$/.test(value);
  }
  if (state && state.pageUrl === pageUrl) {
    // `state` is page-readable and may have been replaced with a Proxy or
    // accessor-bearing object between polls. Snapshot every value used for
    // the native return exactly once: validating one getter result and then
    // reading it again would let a hostile getter substitute an arbitrarily
    // large string for Wry to serialize before Rust can apply its bound.
    var rgba = state.rgba;
    var done = state.done;
    var completeRebuildUsed = state.completeRebuildUsed;
    var initialReadyState = state.initialReadyState;
    if (validRgba(rgba)) return rgba;
    if (rgba == null && typeof done === 'boolean') {
      // UrlChanged can run while the parser has not inserted icon links yet.
      // Once that bounded attempt has finished, permit exactly one fresh
      // candidate scan after the same document transitions to complete.
      if (done && completeRebuildUsed === false &&
          (initialReadyState === 'loading' ||
           initialReadyState === 'interactive') &&
          readyState === 'complete') {
        rebuildAfterComplete = true;
      } else {
        return null;
      }
    }
    // Invalid page-mutable state is ignored and rebuilt below. In
    // particular, never echo an attacker-sized string to native code.
    state = null;
  }

  state = Object.create(null);
  state.pageUrl = pageUrl;
  state.rgba = null;
  state.done = false;
  state.initialReadyState = readyState;
  state.completeRebuildUsed = rebuildAfterComplete;
  state.index = 0;
  state.candidates = [];
  globalThis[key] = state;

  var ranked = [];
  var links = document.querySelectorAll('link[rel~="icon"]');
  for (var i = 0; i < links.length && i < 32; i++) {
    var href = links[i].getAttribute('href');
    if (!href || href.length > 2048) continue;
    try {
      var candidate = new URL(href, document.location.href);
      if ((candidate.protocol !== 'http:' && candidate.protocol !== 'https:') ||
          candidate.origin !== document.location.origin ||
          candidate.username || candidate.password ||
          candidate.href.length > 2048) continue;
      var declared = parseInt((links[i].getAttribute('sizes') || '').split('x')[0], 10);
      var size = Number.isFinite(declared) && declared > 0 ? declared : 32;
      ranked.push([Math.abs(size - 32), candidate.href]);
    } catch (_) {}
  }
  ranked.sort(function(a, b) { return a[0] - b[0]; });
  for (var j = 0; j < ranked.length && state.candidates.length < 4; j++) {
    if (state.candidates.indexOf(ranked[j][1]) === -1) state.candidates.push(ranked[j][1]);
  }
  try {
    var fallback = new URL('/favicon.ico', document.location.origin).href;
    if (fallback.length <= 2048 && state.candidates.indexOf(fallback) === -1) {
      state.candidates.push(fallback);
    }
  } catch (_) {}

  function next() {
    if (state.index >= state.candidates.length) {
      state.done = true;
      return;
    }
    var image = new Image();
    image.decoding = 'async';
    image.onload = function() {
      try {
        var width = image.naturalWidth;
        var height = image.naturalHeight;
        if (!Number.isFinite(width) || !Number.isFinite(height) ||
            width < 1 || height < 1 || width > 16384 || height > 16384) {
          next();
          return;
        }
        var canvas = document.createElement('canvas');
        canvas.width = 32;
        canvas.height = 32;
        var context = canvas.getContext('2d', {alpha: true, willReadFrequently: true});
        if (!context) {
          state.done = true;
          return;
        }
        context.clearRect(0, 0, 32, 32);
        var scale = Math.min(32 / width, 32 / height);
        var drawWidth = Math.max(1, Math.round(width * scale));
        var drawHeight = Math.max(1, Math.round(height * scale));
        context.drawImage(
          image,
          Math.floor((32 - drawWidth) / 2),
          Math.floor((32 - drawHeight) / 2),
          drawWidth,
          drawHeight
        );
        var bytes = context.getImageData(0, 0, 32, 32).data;
        var binary = '';
        for (var k = 0; k < bytes.length; k++) binary += String.fromCharCode(bytes[k]);
        state.rgba = btoa(binary);
        state.done = true;
      } catch (_) {
        next();
      }
    };
    image.onerror = next;
    image.src = state.candidates[state.index++];
  }
  next();
  return null;
})()"#;

// Capture native string/DOM intrinsics before page script can replace them.
// The locked function returns one primitive string: truncation flag + at most
// the requested number of UTF-16 units. This gives native JSON conversion a
// calculable bound and never traverses a page-controlled object/toJSON hook.
pub(super) const EXTRACT_HTML_BOOTSTRAP_JS: &str = r#"(function(){
  'use strict';
  var key = '__zephium_extract_html_v1__';
  if (Object.prototype.hasOwnProperty.call(globalThis, key)) return;
  try {
    var apply = Reflect.apply;
    var slice = String.prototype.slice;
    var descriptor = Object.getOwnPropertyDescriptor(Element.prototype, 'outerHTML');
    var getter = descriptor && descriptor.get;
    if (typeof apply !== 'function' || typeof slice !== 'function' || typeof getter !== 'function') return;
    var extract = function(max) {
      try {
        var root = document.documentElement;
        var html = root ? apply(getter, root, []) : '';
        if (typeof html !== 'string') return null;
        return (html.length > max ? '1' : '0') + apply(slice, html, [0, max]);
      } catch (_) { return null; }
    };
    Object.defineProperty(globalThis, key, {
      value: extract, writable: false, configurable: false, enumerable: false
    });
  } catch (_) {}
})()"#;

pub(super) const MAX_HTML_CHARS: usize = 2 * 1024 * 1024;
// JSON may encode each UTF-16 unit as six ASCII bytes (`\uXXXX`), plus the
// one-unit flag and surrounding quotes. The bootstrap enforces this before
// the platform creates the callback string.
pub(super) const MAX_HTML_RESULT_BYTES: usize = (MAX_HTML_CHARS + 1) * 6 + 2;
pub(super) const EXTRACT_HTML_JS: &str = "(function(){try{var f=globalThis.__zephium_extract_html_v1__;return typeof f==='function'?f(__MAX__):null}catch(_){return null}})()";

pub(super) fn decode_favicon_eval_result(result: &str) -> Option<Vec<u8>> {
    // Wry returns a JSON serialization of the primitive JavaScript string.
    // Foundation is allowed to spell every base64 solidus as `\/`, while
    // JavaScriptCore/Chromium commonly leave it unescaped. Bound the raw JSON
    // before parsing, then require the exact canonical base64 payload and
    // fixed decoded raster. This accepts both native spellings without
    // widening the callback to objects or attacker-sized strings.
    const MAX_RESULT_BYTES: usize = zephium_core::icon::RGBA32_BASE64_BYTES * 2 + 2;
    if result.len() > MAX_RESULT_BYTES {
        return None;
    }
    let encoded = serde_json::from_str::<String>(result).ok()?;
    zephium_core::icon::decode_rgba32(&encoded)
}

fn style_script(style: &UserStyle) -> UserScript {
    // WebView2 runs document-start scripts before <html> exists; WebKit does
    // not. The observer path injects the instant the root appears.
    let source = format!(
        "(function(){{var css={};function add(){{var s=document.createElement('style');s.textContent=css;(document.head||document.documentElement).appendChild(s)}}if(document.head||document.documentElement){{add()}}else{{new MutationObserver(function(_,o){{if(document.documentElement){{o.disconnect();add()}}}}).observe(document,{{childList:true}})}}}})()",
        // Serializing a valid Rust string as one JSON string has no semantic
        // failure mode. Treat an impossible formatter failure as an empty
        // literal rather than interpolating unquoted source.
        serde_json::to_string(style.css.as_ref()).unwrap_or_else(|_| "\"\"".to_string())
    );
    let world = match style.owner {
        ScriptOwner::Builtin => World::Page,
        ScriptOwner::Principal(principal) => World::Isolated(principal),
    };
    UserScript {
        id: style.id,
        owner: style.owner,
        source: source.into(),
        world,
        matches: style.matches.clone(),
        run_at: RunAt::DocumentStart,
        all_frames: style.all_frames,
    }
}

fn builtin_script(id: u128, source: &str, all_frames: bool) -> UserScript {
    UserScript {
        id: ScriptId::from(id),
        owner: ScriptOwner::Builtin,
        source: source.into(),
        world: World::Page,
        matches: MatchSet::all_urls(),
        run_at: RunAt::DocumentStart,
        all_frames,
    }
}

#[derive(Clone, Copy)]
struct ProtectedScriptSpec {
    id: u128,
    source: &'static str,
    all_frames: bool,
}

const PROTECTED_SCRIPT_SPECS: [ProtectedScriptSpec;
    4 + cfg!(any(target_os = "macos", target_os = "windows")) as usize] = [
    ProtectedScriptSpec {
        id: 6,
        source: include_str!("content_style.js"),
        all_frames: false,
    },
    ProtectedScriptSpec {
        id: 1,
        source: DISCARD_SAFETY_BOOTSTRAP_JS,
        all_frames: false,
    },
    ProtectedScriptSpec {
        id: 2,
        source: EXTRACT_HTML_BOOTSTRAP_JS,
        all_frames: false,
    },
    ProtectedScriptSpec {
        id: 3,
        source: crate::PAGE_PRINT_DENY_SCRIPT,
        all_frames: true,
    },
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    ProtectedScriptSpec {
        // The desktop's global page style already owns builtin ID 4.
        id: 5,
        source: zephium_webext::store::PAGE_SCRIPT,
        all_frames: false,
    },
];

fn protected_scripts() -> &'static [UserScript; PROTECTED_SCRIPT_SPECS.len()] {
    // These descriptors contain only immutable, profile-independent source
    // and matching rules. Each view still gets its own native registrations
    // and document state; sharing Rust source buffers grants no shared world.
    static SCRIPTS: std::sync::OnceLock<[UserScript; PROTECTED_SCRIPT_SPECS.len()]> =
        std::sync::OnceLock::new();
    SCRIPTS.get_or_init(|| {
        PROTECTED_SCRIPT_SPECS.map(|spec| builtin_script(spec.id, spec.source, spec.all_frames))
    })
}

fn is_protected_builtin_id(id: ScriptId) -> bool {
    PROTECTED_SCRIPT_SPECS
        .iter()
        .any(|spec| id == ScriptId::from(spec.id))
}

struct ScopedUserContent {
    generation: UserContentGeneration,
    scripts: Vec<UserScript>,
    retained_budget_bytes: usize,
}

#[derive(Default)]
pub(super) struct UserContentRegistry {
    scopes: HashMap<ContentScope, ScopedUserContent>,
    high_water: HashMap<ContentScope, UserContentGeneration>,
}

impl UserContentRegistry {
    fn generation(&self, scope: ContentScope) -> Option<UserContentGeneration> {
        self.scopes.get(&scope).map(|entry| entry.generation)
    }

    fn admit_generation(
        &mut self,
        scope: ContentScope,
        generation: UserContentGeneration,
    ) -> Result<(), UserContentApplyFailure> {
        if scope == ContentScope::Global {
            // Global injected content is a host bootstrap authority, not an
            // extension/userscript scope. Keeping it out of the runtime port
            // prevents one profile-mapping bug from crossing profile and
            // private-browsing boundaries.
            return Err(UserContentApplyFailure::ReservedScope);
        }
        if self
            .high_water
            .get(&scope)
            .copied()
            .is_some_and(|current| generation <= current)
        {
            return Err(UserContentApplyFailure::StaleGeneration);
        }
        if !self.high_water.contains_key(&scope)
            && self
                .high_water
                .keys()
                .filter(|scope| matches!(scope, ContentScope::Profile(_)))
                .count()
                >= zephium_core::session::MAX_SESSION_PROFILES
        {
            return Err(UserContentApplyFailure::TooManyScopes);
        }
        self.high_water.insert(scope, generation);
        Ok(())
    }

    fn validate_replacement(
        &self,
        scope: ContentScope,
        content: &UserContent,
    ) -> Result<(), UserContentApplyFailure> {
        content.validate()?;
        let candidate_budget = content
            .retained_budget_bytes()
            .ok_or(UserContentApplyFailure::TotalRetainedTooLarge)?;
        let current_budget = self
            .scopes
            .values()
            .try_fold(0_usize, |total, entry| {
                total.checked_add(entry.retained_budget_bytes)
            })
            .ok_or(UserContentApplyFailure::ProcessBudgetExceeded)?;
        let replaced_budget = self
            .scopes
            .get(&scope)
            .map_or(0, |entry| entry.retained_budget_bytes);
        if current_budget
            .checked_sub(replaced_budget)
            .and_then(|bytes| bytes.checked_add(candidate_budget))
            .is_none_or(|bytes| {
                bytes > zephium_core::ports::engine::MAX_USER_CONTENT_RETAINED_BYTES_PROCESS
            })
        {
            return Err(UserContentApplyFailure::ProcessBudgetExceeded);
        }
        let host_only_refusals = content
            .scripts
            .iter()
            .filter(|script| script.owner == ScriptOwner::Builtin)
            .map(|script| UserScriptRefusal {
                registration: script.key(),
                reason: UserScriptRefusalReason::HostOnlyOwner,
            })
            .chain(
                content
                    .styles
                    .iter()
                    .filter(|style| style.owner == ScriptOwner::Builtin)
                    .map(|style| UserScriptRefusal {
                        registration: style.key(),
                        reason: UserScriptRefusalReason::HostOnlyOwner,
                    }),
            )
            .collect::<Vec<_>>();
        if !host_only_refusals.is_empty() {
            return Err(UserContentApplyFailure::Scripts(host_only_refusals));
        }
        validate_reserved_ids(content)?;
        Ok(())
    }

    fn validate_initial_global(content: &UserContent) -> Result<(), UserContentApplyFailure> {
        content.validate()?;
        let invalid_scope_refusals = content
            .scripts
            .iter()
            .filter(|script| script.owner != ScriptOwner::Builtin)
            .map(|script| UserScriptRefusal {
                registration: script.key(),
                reason: UserScriptRefusalReason::InvalidScope,
            })
            .chain(
                content
                    .styles
                    .iter()
                    .filter(|style| style.owner != ScriptOwner::Builtin)
                    .map(|style| UserScriptRefusal {
                        registration: style.key(),
                        reason: UserScriptRefusalReason::InvalidScope,
                    }),
            )
            .collect::<Vec<_>>();
        if !invalid_scope_refusals.is_empty() {
            return Err(UserContentApplyFailure::Scripts(invalid_scope_refusals));
        }
        validate_reserved_ids(content)?;
        let refusals = platform_refusals(content);
        if refusals.is_empty() {
            Ok(())
        } else {
            Err(UserContentApplyFailure::Scripts(refusals))
        }
    }

    pub(super) fn remove_profile(&mut self, profile: zephium_core::ids::ProfileId) {
        self.scopes.remove(&ContentScope::Profile(profile));
        self.high_water.remove(&ContentScope::Profile(profile));
    }

    fn commit(
        &mut self,
        scope: ContentScope,
        generation: UserContentGeneration,
        content: UserContent,
    ) {
        let retained_budget_bytes = content
            .retained_budget_bytes()
            .expect("validated user content has a bounded retained-memory charge");
        let mut scripts = content.scripts;
        scripts.extend(content.styles.iter().map(style_script));
        self.scopes.insert(
            scope,
            ScopedUserContent {
                generation,
                scripts,
                retained_budget_bytes,
            },
        );
    }

    #[cfg(test)]
    fn replace(
        &mut self,
        scope: ContentScope,
        generation: UserContentGeneration,
        content: UserContent,
    ) -> Result<(), UserContentApplyFailure> {
        self.admit_generation(scope, generation)?;
        self.validate_replacement(scope, &content)?;
        self.commit(scope, generation, content);
        Ok(())
    }

    pub(super) fn with_initial_global(
        generation: UserContentGeneration,
        content: UserContent,
    ) -> Result<Self, UserContentApplyFailure> {
        Self::validate_initial_global(&content)?;
        let mut registry = Self::default();
        registry.high_water.insert(ContentScope::Global, generation);
        registry.commit(ContentScope::Global, generation, content);
        Ok(registry)
    }

    fn scripts_for(&self, partition: Partition) -> Vec<UserScript> {
        let scopes = [
            ContentScope::Global,
            ContentScope::Profile(partition.profile()),
        ]
        .map(|scope| self.scopes.get(&scope));
        let capacity = PROTECTED_SCRIPT_SPECS.len()
            + scopes
                .iter()
                .flatten()
                .map(|entry| entry.scripts.len())
                .sum::<usize>();
        let mut out = Vec::with_capacity(capacity);
        out.extend(protected_scripts().iter().cloned());
        for entry in scopes.into_iter().flatten() {
            out.extend(entry.scripts.iter().cloned());
        }
        out
    }
}

fn validate_reserved_ids(content: &UserContent) -> Result<(), UserContentApplyFailure> {
    let reserved_refusals: Vec<_> = content
        .scripts
        .iter()
        .map(UserScript::key)
        .chain(content.styles.iter().map(UserStyle::key))
        .filter(|key| key.owner == ScriptOwner::Builtin && is_protected_builtin_id(key.id))
        .map(|registration| UserScriptRefusal {
            registration,
            reason: UserScriptRefusalReason::ProtectedRegistration,
        })
        .collect();
    if !reserved_refusals.is_empty() {
        return Err(UserContentApplyFailure::Scripts(reserved_refusals));
    }
    Ok(())
}

fn platform_refusals(content: &UserContent) -> Vec<UserScriptRefusal> {
    content
        .scripts
        .iter()
        .filter_map(|script| {
            crate::platform::imp::user_script_refusal(script).map(|reason| UserScriptRefusal {
                registration: script.key(),
                reason,
            })
        })
        .chain(content.styles.iter().filter_map(|style| {
            crate::platform::imp::user_style_refusal(style).map(|reason| UserScriptRefusal {
                registration: style.key(),
                reason,
            })
        }))
        .collect()
}

impl EngineHost {
    pub(super) fn scripts_for(&self, partition: Partition) -> Vec<UserScript> {
        self.user_content.scripts_for(partition)
    }

    pub(crate) fn set_user_content(
        &mut self,
        scope: ContentScope,
        generation: UserContentGeneration,
        content: UserContent,
    ) {
        let previous = self.user_content.generation(scope);
        let affects_live_view = self
            .partitions
            .values()
            .copied()
            .any(|partition| match scope {
                ContentScope::Global => true,
                ContentScope::Profile(profile) => partition.profile() == profile,
            });
        let result = self
            .user_content
            .admit_generation(scope, generation)
            .and_then(|()| self.user_content.validate_replacement(scope, &content))
            .and_then(|()| {
                let refusals = platform_refusals(&content);
                if !refusals.is_empty() {
                    return Err(UserContentApplyFailure::Scripts(refusals));
                }
                if affects_live_view {
                    // Phase 1's platform mutation adapters replace this typed
                    // refusal. Retaining is the only truthful result until
                    // every affected live controller can be changed atomically
                    // without losing protected registrations.
                    return Err(UserContentApplyFailure::UnsupportedPlatform);
                }
                // A matching spare is disposable and cannot retain the prior
                // registration set. An unrelated profile's spare is unaffected
                // by a profile-scoped replacement and need not be churned.
                let spare_is_affected = self.spare.as_ref().is_some_and(|spare| match scope {
                    ContentScope::Global => true,
                    ContentScope::Profile(profile) => spare.partition.profile() == profile,
                });
                if spare_is_affected {
                    self.spare = None;
                }
                self.user_content.commit(scope, generation, content);
                Ok(())
            });
        let settlement = match result {
            Ok(()) => UserContentSettlement::Applied { generation },
            Err(failure) => match previous {
                Some(generation) => UserContentSettlement::Retained {
                    generation,
                    failure,
                },
                None => UserContentSettlement::Unavailable { failure },
            },
        };
        self.sink.emit(EngineEvent::UserContentSettled {
            scope,
            requested: generation,
            settlement,
        });
    }

    pub(crate) fn set_shortcuts(&mut self, shortcuts: Vec<Shortcut>) {
        self.shortcuts = shortcuts;
        self.spare = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zephium_core::ids::{ExtensionInstallId, ProfileId, UserscriptId};
    use zephium_core::ports::engine::{ScriptPrincipal, UserScriptRefusalReason};

    fn generation(value: u64) -> UserContentGeneration {
        UserContentGeneration::new(value).unwrap()
    }

    fn test_script(id: u128) -> UserScript {
        let principal = ScriptPrincipal::Userscript(UserscriptId::from(id + 100));
        UserScript {
            id: ScriptId::from(id),
            owner: ScriptOwner::Principal(principal),
            source: "globalThis.__zephium_test__ = true".into(),
            world: World::Isolated(principal),
            matches: MatchSet::all_urls(),
            run_at: RunAt::DocumentStart,
            all_frames: false,
        }
    }

    #[test]
    fn protected_registrations_survive_every_registry_transition() {
        let profile = ProfileId::from(17);
        let partition = Partition::Persistent(profile);
        let mut registry = UserContentRegistry::default();

        for (generation, scripts) in [
            (1, vec![test_script(10)]),
            (2, vec![test_script(11), test_script(12)]),
            (3, Vec::new()),
        ] {
            registry
                .replace(
                    ContentScope::Profile(profile),
                    self::generation(generation),
                    UserContent {
                        scripts,
                        styles: Vec::new(),
                    },
                )
                .unwrap();
            let resolved = registry.scripts_for(partition);
            let expected = PROTECTED_SCRIPT_SPECS
                .iter()
                .map(|spec| ScriptId::from(spec.id))
                .collect::<Vec<_>>();
            assert_eq!(
                resolved
                    .iter()
                    .take(PROTECTED_SCRIPT_SPECS.len())
                    .map(|script| script.id)
                    .collect::<Vec<_>>(),
                expected
            );
            for (index, spec) in PROTECTED_SCRIPT_SPECS.iter().enumerate() {
                assert_eq!(resolved[index].run_at, RunAt::DocumentStart);
                assert_eq!(resolved[index].world, World::Page);
                assert_eq!(resolved[index].all_frames, spec.all_frames);
                assert_eq!(
                    resolved
                        .iter()
                        .filter(|script| script.id == ScriptId::from(spec.id))
                        .map(|script| script.source.as_ref())
                        .collect::<Vec<_>>(),
                    vec![spec.source]
                );
            }
        }
    }

    #[test]
    fn every_protected_descriptor_reserves_its_builtin_registration_id() {
        for spec in PROTECTED_SCRIPT_SPECS {
            let mut protected = test_script(spec.id);
            protected.owner = ScriptOwner::Builtin;
            protected.world = World::Page;
            let protected_key = protected.key();
            let failure = UserContentRegistry::with_initial_global(
                generation(1),
                UserContent {
                    scripts: vec![protected],
                    styles: Vec::new(),
                },
            )
            .err()
            .unwrap();
            assert!(matches!(
                failure,
                UserContentApplyFailure::Scripts(refusals)
                    if refusals.iter().any(|refusal| refusal.registration == protected_key
                        && refusal.reason == UserScriptRefusalReason::ProtectedRegistration)
            ));
        }
    }

    #[test]
    fn protected_numeric_ids_remain_available_to_distinct_principals() {
        let profile = ProfileId::from(18);
        let mut registry = UserContentRegistry::default();
        let scripts = PROTECTED_SCRIPT_SPECS
            .iter()
            .map(|spec| test_script(spec.id))
            .collect();
        registry
            .replace(
                ContentScope::Profile(profile),
                generation(1),
                UserContent {
                    scripts,
                    styles: Vec::new(),
                },
            )
            .unwrap();

        let resolved = registry.scripts_for(Partition::Persistent(profile));
        for spec in PROTECTED_SCRIPT_SPECS {
            assert_eq!(
                resolved
                    .iter()
                    .filter(|script| script.id == ScriptId::from(spec.id))
                    .count(),
                2
            );
        }
    }

    #[test]
    fn stale_or_invalid_replacement_retains_the_prior_generation() {
        let profile = ProfileId::from(19);
        let mut registry = UserContentRegistry::default();
        registry
            .replace(
                ContentScope::Profile(profile),
                generation(2),
                UserContent {
                    scripts: vec![test_script(20)],
                    styles: Vec::new(),
                },
            )
            .unwrap();
        assert_eq!(
            registry.replace(
                ContentScope::Profile(profile),
                generation(2),
                UserContent::default(),
            ),
            Err(UserContentApplyFailure::StaleGeneration)
        );
        assert_eq!(
            registry.generation(ContentScope::Profile(profile)),
            Some(generation(2))
        );
        assert!(registry
            .scripts_for(Partition::Persistent(profile))
            .iter()
            .any(|script| script.id == ScriptId::from(20)));
    }

    #[test]
    fn runtime_scope_and_owner_authority_are_fail_closed() {
        let profile = ProfileId::from(20);
        let mut registry = UserContentRegistry::default();
        assert_eq!(
            registry.replace(ContentScope::Global, generation(1), UserContent::default()),
            Err(UserContentApplyFailure::ReservedScope)
        );

        let host_script = builtin_script(100, "globalThis.hostOnly = true", false);
        let host_key = host_script.key();
        let failure = registry
            .replace(
                ContentScope::Profile(profile),
                generation(1),
                UserContent {
                    scripts: vec![host_script],
                    styles: Vec::new(),
                },
            )
            .unwrap_err();
        assert!(matches!(
            failure,
            UserContentApplyFailure::Scripts(refusals)
                if refusals == vec![UserScriptRefusal {
                    registration: host_key,
                    reason: UserScriptRefusalReason::HostOnlyOwner,
                }]
        ));
    }

    #[test]
    fn initial_global_rejects_principal_content() {
        let principal = test_script(101);
        let key = principal.key();
        let failure = UserContentRegistry::with_initial_global(
            generation(1),
            UserContent {
                scripts: vec![principal],
                styles: Vec::new(),
            },
        )
        .err()
        .unwrap();
        assert!(matches!(
            failure,
            UserContentApplyFailure::Scripts(refusals)
                if refusals == vec![UserScriptRefusal {
                    registration: key,
                    reason: UserScriptRefusalReason::InvalidScope,
                }]
        ));
    }

    #[test]
    fn profile_scope_capacity_is_bounded_and_removal_releases_it() {
        let mut registry = UserContentRegistry::default();
        for value in 1..=zephium_core::session::MAX_SESSION_PROFILES as u128 {
            registry
                .replace(
                    ContentScope::Profile(ProfileId::from(value)),
                    generation(1),
                    UserContent::default(),
                )
                .unwrap();
        }
        let extra = ProfileId::from(10_000);
        assert_eq!(
            registry.replace(
                ContentScope::Profile(extra),
                generation(1),
                UserContent::default(),
            ),
            Err(UserContentApplyFailure::TooManyScopes)
        );

        let released = ProfileId::from(1);
        registry.remove_profile(released);
        assert_eq!(registry.generation(ContentScope::Profile(released)), None);
        registry
            .replace(
                ContentScope::Profile(extra),
                generation(2),
                UserContent::default(),
            )
            .unwrap();
    }

    #[test]
    fn failed_newer_candidate_advances_the_generation_high_water_mark() {
        let profile = ProfileId::from(21);
        let mut registry = UserContentRegistry::default();
        registry
            .replace(
                ContentScope::Profile(profile),
                generation(2),
                UserContent {
                    scripts: vec![test_script(200)],
                    styles: Vec::new(),
                },
            )
            .unwrap();
        let failure = registry
            .replace(
                ContentScope::Profile(profile),
                generation(4),
                UserContent {
                    scripts: vec![builtin_script(201, "void 0", false)],
                    styles: Vec::new(),
                },
            )
            .unwrap_err();
        assert!(matches!(failure, UserContentApplyFailure::Scripts(_)));
        assert_eq!(
            registry.replace(
                ContentScope::Profile(profile),
                generation(3),
                UserContent::default(),
            ),
            Err(UserContentApplyFailure::StaleGeneration)
        );
        assert_eq!(
            registry.generation(ContentScope::Profile(profile)),
            Some(generation(2))
        );
    }

    #[test]
    fn immutable_host_sources_are_shared_across_view_snapshots() {
        let registry = UserContentRegistry::with_initial_global(
            generation(1),
            UserContent {
                scripts: Vec::new(),
                styles: vec![UserStyle {
                    id: ScriptId::from(4),
                    owner: ScriptOwner::Builtin,
                    css: "html { color: black; }".into(),
                    matches: MatchSet::all_urls(),
                    all_frames: true,
                }],
            },
        )
        .unwrap();
        let partition = Partition::Persistent(ProfileId::from(22));
        let first = registry.scripts_for(partition);
        let second = registry.scripts_for(partition);
        let private = registry.scripts_for(Partition::Ephemeral(ProfileId::from(23)));
        for index in 0..PROTECTED_SCRIPT_SPECS.len() {
            assert!(std::sync::Arc::ptr_eq(
                &first[index].source,
                &second[index].source
            ));
            assert!(std::sync::Arc::ptr_eq(
                &first[index].source,
                &private[index].source
            ));
            assert_eq!(first[index].owner, ScriptOwner::Builtin);
            assert_eq!(
                first[index].all_frames,
                PROTECTED_SCRIPT_SPECS[index].all_frames
            );
        }
        let first_style = first
            .iter()
            .find(|script| script.id == ScriptId::from(4))
            .unwrap();
        let second_style = second
            .iter()
            .find(|script| script.id == ScriptId::from(4))
            .unwrap();
        assert!(std::sync::Arc::ptr_eq(
            &first_style.source,
            &second_style.source
        ));
    }

    #[test]
    fn retained_registry_has_a_hard_process_budget() {
        use zephium_core::ports::engine::{
            ScriptPrincipal, MAX_USER_CONTENT_RETAINED_BYTES_PROCESS, MAX_USER_SCRIPT_BYTES,
        };

        let source: std::sync::Arc<str> = "x".repeat(MAX_USER_SCRIPT_BYTES).into();
        let make_content = |seed: u128| {
            let mut scripts = Vec::new();
            for owner_index in 0..4_u128 {
                let principal = ScriptPrincipal::Extension(ExtensionInstallId::from(
                    seed * 100 + owner_index + 1,
                ));
                for script_index in 0..2_u128 {
                    scripts.push(UserScript {
                        id: ScriptId::from(owner_index * 10 + script_index + 1),
                        owner: ScriptOwner::Principal(principal),
                        source: source.clone(),
                        world: World::Isolated(principal),
                        matches: MatchSet::all_urls(),
                        run_at: RunAt::DocumentStart,
                        all_frames: false,
                    });
                }
            }
            UserContent {
                scripts,
                styles: Vec::new(),
            }
        };

        let sample = make_content(1);
        sample.validate().unwrap();
        let per_scope = sample.retained_budget_bytes().unwrap();
        let admitted = MAX_USER_CONTENT_RETAINED_BYTES_PROCESS / per_scope;
        assert!(admitted > 0);
        let mut registry = UserContentRegistry::default();
        for index in 0..admitted {
            registry
                .replace(
                    ContentScope::Profile(ProfileId::from(index as u128 + 1)),
                    generation(1),
                    make_content(index as u128 + 1),
                )
                .unwrap();
        }
        assert_eq!(
            registry.replace(
                ContentScope::Profile(ProfileId::from(admitted as u128 + 1)),
                generation(1),
                make_content(admitted as u128 + 1),
            ),
            Err(UserContentApplyFailure::ProcessBudgetExceeded)
        );
    }

    #[test]
    fn principal_content_is_refused_until_exact_pre_source_match_enforcement_exists() {
        let content = UserContent {
            scripts: vec![test_script(300)],
            styles: Vec::new(),
        };
        assert_eq!(
            platform_refusals(&content),
            vec![UserScriptRefusal {
                registration: content.scripts[0].key(),
                #[cfg(target_os = "windows")]
                reason: UserScriptRefusalReason::UnsupportedWorld,
                #[cfg(not(target_os = "windows"))]
                reason: UserScriptRefusalReason::UnsupportedMatchSet,
            }]
        );
    }

    #[test]
    fn discard_bootstrap_bounds_and_attests_shadow_root_observation() {
        for required in [
            "MAX_SHADOW_ROOTS = 256",
            "Element.prototype",
            "originalAttachShadow",
            "elementProto.attachShadow !== wrappedAttachShadow",
            "rememberShadowRoot",
            "shadowRoots.length >= MAX_SHADOW_ROOTS",
            "Document.prototype.createTreeWalker",
            "new WeakReference(root)",
            "snapshotElements.length >= MAX_TRACKED",
            "template[shadowrootmode]",
            "collect('input,textarea,select')",
            "collect('[contenteditable]",
            "collect('audio,video')",
        ] {
            assert!(
                DISCARD_SAFETY_BOOTSTRAP_JS.contains(required),
                "discard bootstrap lost required shadow-root invariant: {required}"
            );
        }
    }

    #[test]
    fn html_extraction_uses_locked_intrinsics_and_a_bounded_primitive() {
        for required in [
            "var apply = Reflect.apply",
            "var slice = String.prototype.slice",
            "Object.getOwnPropertyDescriptor(Element.prototype, 'outerHTML')",
            "writable: false, configurable: false",
            "html.length > max ? '1' : '0'",
            "apply(slice, html, [0, max])",
        ] {
            assert!(
                EXTRACT_HTML_BOOTSTRAP_JS.contains(required),
                "HTML bootstrap lost native-bound invariant: {required}"
            );
        }
        assert!(!EXTRACT_HTML_JS.contains("return {"));
        assert_eq!(MAX_HTML_RESULT_BYTES, (MAX_HTML_CHARS + 1) * 6 + 2);
    }

    #[test]
    fn page_print_guard_locks_all_known_scripted_print_lookup_paths() {
        for required in [
            "apply(defineProperty, Object, [Window.prototype, 'print'",
            "apply(defineProperty, Object, [globalThis, 'print'",
            "apply(getOwnPropertyDescriptor, Object, [Document.prototype, 'execCommand'])",
            "apply(defineProperty, Object, [Document.prototype, 'execCommand'",
            "apply(defineProperty, Object, [document, 'execCommand'",
            "primitive = apply(string, undefined, [command])",
            "apply(trim, primitive",
            "apply(toLowerCase",
            "normalized === 'print'",
            "return apply(execCommand, this, [primitive",
            "writable: false",
            "configurable: false",
        ] {
            assert!(
                crate::PAGE_PRINT_DENY_SCRIPT.contains(required),
                "page print guard lost required invariant: {required}"
            );
        }
        assert!(!crate::PAGE_PRINT_DENY_SCRIPT.contains("window.print()"));
        assert!(!crate::PAGE_PRINT_DENY_SCRIPT.contains("apply(execCommand, this, arguments)"));
    }

    #[test]
    fn page_print_guard_coerces_exec_command_once_before_delegating() {
        // A hostile object may alternate its string conversion between a
        // harmless command and `print`. Authorize and delegate the exact same
        // captured primitive so Web IDL cannot invoke it a second time.
        assert_eq!(
            crate::PAGE_PRINT_DENY_SCRIPT
                .matches("apply(string, undefined, [command])")
                .count(),
            1
        );
        assert!(crate::PAGE_PRINT_DENY_SCRIPT
            .contains("return apply(execCommand, this, [primitive, arguments[1]"));
    }

    #[test]
    fn favicon_script_bounds_page_mutable_state_before_returning_it() {
        assert!(FAVICON_JS.contains("value.length === 5464"));
        assert!(FAVICON_JS.contains("[A-Za-z0-9+/]{5462}=="));
        assert!(FAVICON_JS.contains("var rgba = state.rgba;"));
        assert!(FAVICON_JS.contains("if (validRgba(rgba)) return rgba;"));
        assert!(!FAVICON_JS.contains("return state.rgba"));
        let length_check = FAVICON_JS.find("value.length === 5464").unwrap();
        let host_return = FAVICON_JS.find("return rgba").unwrap();
        assert!(length_check < host_return);
        assert!(!FAVICON_JS.contains("return {"));
        assert!(!FAVICON_JS.contains("rgba: state.rgba"));
    }

    #[test]
    fn favicon_callback_accepts_bounded_native_json_spellings_of_the_canonical_string() {
        let rgba: Vec<u8> = (0..zephium_core::icon::RGBA32_BYTES)
            .map(|index| (index % 251) as u8)
            .collect();
        let encoded = zephium_core::icon::encode_rgba32(&rgba).unwrap();
        assert!(encoded.contains('/'));
        let serialized = serde_json::to_string(&encoded).unwrap();
        let foundation_serialized = serialized.replace('/', "\\/");
        assert_eq!(decode_favicon_eval_result(&serialized), Some(rgba.clone()));
        assert_eq!(
            decode_favicon_eval_result(&foundation_serialized),
            Some(rgba)
        );
        assert!(decode_favicon_eval_result("null").is_none());
        assert!(decode_favicon_eval_result(&encoded).is_none());
        assert!(decode_favicon_eval_result(&format!("\"{encoded}x\"")).is_none());
        assert!(decode_favicon_eval_result(&format!(
            "\"{}\"",
            "A".repeat(zephium_core::icon::RGBA32_BASE64_BYTES * 2 + 1)
        ))
        .is_none());
    }

    #[test]
    fn favicon_script_rebuilds_candidates_once_after_document_completion() {
        let ready_state = FAVICON_JS
            .find("var readyState = document.readyState")
            .unwrap();
        let completed_retry = FAVICON_JS.find("completeRebuildUsed === false").unwrap();
        let early_terminal_return = FAVICON_JS.find("return null").unwrap();
        let retry_consumed = FAVICON_JS
            .find("state.completeRebuildUsed = rebuildAfterComplete")
            .unwrap();
        let candidate_scan = FAVICON_JS
            .find("document.querySelectorAll('link[rel~=\"icon\"]')")
            .unwrap();
        assert!(ready_state < completed_retry);
        assert!(completed_retry < early_terminal_return);
        assert!(early_terminal_return < retry_consumed);
        assert!(retry_consumed < candidate_scan);
    }
}
