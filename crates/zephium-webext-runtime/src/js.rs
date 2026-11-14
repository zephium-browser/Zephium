//! The engine-neutral JavaScript binding, as source text for adapters.
//!
//! [`BUNDLE`] is one function expression. An adapter evaluates
//! `(BUNDLE)(entry, surface, bootstrap)` where `surface` is the generated
//! API surface (`js/surface.generated.js`, one JSON value) and `bootstrap`
//! the context's JSON blob:
//!
//! - `"page"`: extension pages and offscreen documents, in the frame's default
//!   world right after the web-process module defined `__zephiumPost` and
//!   `__zephiumRegisterReceiver`, before any page script;
//! - `"content"`: once per document in the install's isolated world, before
//!   any content script; script bundles then call the guard it publishes;
//! - `"native-content"`: eagerly creates the content API after the native
//!   adapter has matched the document; each file then runs at top level;
//! - `"worker"`: first thing in the background worker, before the
//!   extension's service-worker script;
//! - `"owner"`: in the hidden owner page, with the worker start data as
//!   `bootstrap`.
//!
//! [`MATCHER`] is `(data) => boolean` for MAIN-world script bundles, which
//! get no API.

macro_rules! modules {
    () => {
        concat!(
            "const Z={__proto__:null};Z.I=(",
            include_str!("../js/intrinsics.js"),
            ")(globalThis);Z.errors=(",
            include_str!("../js/errors.js"),
            ")(Z);Z.transport=(",
            include_str!("../js/transport.js"),
            ")(Z);Z.events=(",
            include_str!("../js/events.js"),
            ")(Z);Z.ports=(",
            include_str!("../js/ports.js"),
            ")(Z);Z.api=(",
            include_str!("../js/api.js"),
            ")(Z);Z.content=(",
            include_str!("../js/content.js"),
            ")(Z);Z.workerScope=(",
            include_str!("../js/worker_scope.js"),
            ")(Z);"
        )
    };
}

/// Every context's binding; see the module documentation for the entries.
pub const BUNDLE: &str = concat!(
    "(function zephiumWebExtension(entry,surface,bootstrap){'use strict';",
    modules!(),
    "switch(entry){",
    "case'page':return Z.api.page(surface,bootstrap);",
    "case'content':return Z.content.boot(surface,bootstrap);",
    "case'native-content':if(!bootstrap||!['contentScript','userScript'].includes(bootstrap.contextKind))throw new TypeError('Invalid native content context');return Z.api.page(surface,bootstrap);",
    "case'worker':return Z.workerScope.worker(surface,bootstrap);",
    "case'owner':return Z.workerScope.owner(bootstrap);",
    "default:throw new TypeError('Unknown binding entry');}})"
);

/// The content-script match guard alone, for MAIN-world bundles.
pub const MATCHER: &str = concat!(
    "(function zephiumMatcher(data){'use strict';const Z={__proto__:null};Z.I=(",
    include_str!("../js/intrinsics.js"),
    ")(globalThis);Z.content=(",
    include_str!("../js/content.js"),
    ")(Z);return Z.content.matches(data);})"
);
