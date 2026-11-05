//! Public-site controls with no provider or automated input.
#![deny(unsafe_op_in_unsafe_fn, clippy::undocumented_unsafe_blocks)]

use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject, MainThreadOnly};
use objc2_app_kit::{
    NSAccessibility as _, NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSDate, NSError, NSHTTPCookie, NSPoint, NSRect, NSRunLoop, NSSize,
    NSString,
};
use objc2_web_kit::WKContentWorld;
use raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};
use std::{
    cell::RefCell,
    ffi::c_void,
    ptr::NonNull,
    rc::Rc,
    time::{Duration, Instant},
};
use wry::WebViewBuilder;

/// QA-only stderr sink for owned agent-view traces; production modules never print.
pub(crate) fn trace(line: std::fmt::Arguments<'_>) {
    eprintln!("{line}");
}

const OBSERVATION_WINDOW: Duration = Duration::from_secs(30);
const SAMPLE_DEADLINE: Duration = Duration::from_secs(5);
const INTERACTIVE_OBSERVATION_WINDOW: Duration = Duration::from_secs(120);

#[path = "agentic_liveness_fixture.rs"]
mod fixture;

#[path = "agentic_challenge_ax_probe.rs"]
mod challenge_ax;

#[cfg(feature = "native-agentic-semantic-probe")]
#[path = "agentic_construction_probe.rs"]
mod construction;
#[cfg(feature = "native-agentic-semantic-probe")]
pub use construction::run_construction_liveness_probe;
#[cfg(feature = "native-agentic-semantic-probe")]
#[path = "agentic_human_probe.rs"]
mod human;
#[cfg(feature = "native-agentic-semantic-probe")]
pub use human::run_human_takeover_probe;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivenessSite {
    Government,
    GovernmentCanonical,
    Cloudflare,
    AnimationFixture,
    /// A browser search's results page, held by the results-page policy.
    GoogleResults,
}
impl LivenessSite {
    fn url(self) -> &'static str {
        match self {
            Self::Government => "https://travel.state.gov/",
            Self::GovernmentCanonical => "https://travel.state.gov/en.html",
            Self::Cloudflare => "https://www.cloudflare.com/",
            Self::AnimationFixture => "about:blank",
            Self::GoogleResults => "https://www.google.com/search?q=rust+async+runtime&hl=en",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LivenessStage {
    Bare,
    Safari,
    BrowseScripts,
    DocumentGate,
    Hidden,
    OwnedWork,
    OwnedUnthrottled,
    OwnedOffscreenWindow,
    OwnedOffscreenChild,
    OwnedPresented,
    OwnedHosted,
}

enum ProbePage {
    Raw(wry::WebView),
    Owned(super::agent_context::AgentOwnedView),
}
impl ProbePage {
    fn view(&self) -> &wry::WebView {
        match self {
            Self::Raw(view) => view,
            Self::Owned(view) => view.view(),
        }
    }
}

struct Host(Retained<NSView>);
impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::from(&*self.0).cast::<c_void>(),
        ));
        // SAFETY: the host retains this content view longer than the child WebView.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

fn pump() {
    objc2::rc::autoreleasepool(|_| {
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.01));
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            if let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                objc2_app_kit::NSEventMask::Any,
                None,
                objc2_foundation::ns_string!("NSDefaultRunLoopMode"),
                true,
            ) {
                app.sendEvent(&event);
            }
        }
    });
}

const SAMPLE: &str = r#"
const start = performance.now();
let raf = 0, timers = 0, timerMax = 0, timerTotal = 0, stopped = false;
function frame() { if (!stopped) { raf++; requestAnimationFrame(frame); } }
function timer() {
  const before = performance.now();
  setTimeout(() => {
    if (stopped) return;
    const dt = performance.now() - before;
    timers++; timerMax = Math.max(timerMax, dt); timerTotal += dt;
    if (timers < 512) timer();
  }, 0);
}
requestAnimationFrame(frame); timer();
return new Promise(resolve => setTimeout(() => {
  stopped = true;
  const text = (document.body?.innerText || '').slice(0, 65536).toLowerCase();
  const nav = performance.getEntriesByType('navigation')[0];
  resolve(JSON.stringify({
    complete: document.readyState === 'complete', visible: document.visibilityState === 'visible',
    government_content: text.includes('travel advisories') && text.includes('u.s. passports'),
    body_characters: text.length,
    challenge: ['performing security verification','verify you are human','checking your browser','just a moment'].some(x => text.includes(x)),
    light_dom_challenge_frames: Array.from(document.querySelectorAll('iframe')).slice(0, 256).filter(x => { try { return new URL(x.src).hostname === 'challenges.cloudflare.com'; } catch { return false; } }).length,
    light_dom_checkboxes: document.querySelectorAll('input[type=checkbox], [role=checkbox]').length,
    raf, timers, timer_max_ms: Math.round(timerMax), timer_mean_ms: timers ? Math.round(timerTotal / timers) : 0,
    sample_ms: Math.round(performance.now() - start), load_ms: Math.round(nav?.loadEventEnd || 0)
  }));
}, 2000));
"#;

pub fn run_liveness_probe(site: LivenessSite, stage: LivenessStage) -> Result<(), &'static str> {
    run(site, stage, OBSERVATION_WINDOW)
}

pub fn run_interactive_government_probe() -> Result<(), &'static str> {
    run_interactive_liveness_probe(LivenessSite::Government, LivenessStage::Bare)
}

pub fn run_interactive_liveness_probe(
    site: LivenessSite,
    stage: LivenessStage,
) -> Result<(), &'static str> {
    run(site, stage, INTERACTIVE_OBSERVATION_WINDOW)
}

fn run(
    site: LivenessSite,
    stage: LivenessStage,
    observation_window: Duration,
) -> Result<(), &'static str> {
    let fixture = (site == LivenessSite::AnimationFixture)
        .then(fixture::Fixture::start)
        .transpose()?;
    let target = fixture
        .as_ref()
        .map_or_else(|| site.url(), fixture::Fixture::url);
    let mtm = MainThreadMarker::new().ok_or("main_thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();
    // SAFETY: AppKit construction and all accesses are on the main thread.
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(80.0, 80.0), NSSize::new(1280.0, 800.0)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: retained owner handles release after child teardown.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str("Zephium Liveness Probe"));
    window.setAccessibilityIdentifier(Some(&NSString::from_str(challenge_ax::WINDOW_ID)));
    let host = Host(window.contentView().ok_or("content_view")?);
    let owned_stage = matches!(
        stage,
        LivenessStage::OwnedWork
            | LivenessStage::OwnedUnthrottled
            | LivenessStage::OwnedOffscreenWindow
            | LivenessStage::OwnedOffscreenChild
            | LivenessStage::OwnedPresented
            | LivenessStage::OwnedHosted
    );
    let hidden = matches!(
        stage,
        LivenessStage::Hidden | LivenessStage::OwnedWork | LivenessStage::OwnedUnthrottled
    );
    let started = Instant::now();
    let gate = (stage == LivenessStage::DocumentGate)
        .then(crate::platform::work_document_navigation::WorkDocumentNavigation::default);
    let policy_gate = gate.clone();
    let event_gate = gate.clone();
    let mut builder = WebViewBuilder::new()
        .with_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
            size: wry::dpi::LogicalSize::new(1280.0, 800.0).into(),
        })
        .with_url(if gate.is_some() { "about:blank" } else { target }).with_incognito(true).with_visible(!hidden).with_focused(!hidden)
        .with_devtools(true)
        .with_apple_navigation_action_handler(move |target, action| {
            let allowed = policy_gate.as_ref().is_none_or(|gate| gate.allows_apple_action(&target, action.clone()));
            let challenge = url::Url::parse(&target).is_ok_and(|url| url.host_str() == Some("challenges.cloudflare.com"));
            if challenge { eprintln!("liveness_probe site={site:?} stage={stage:?} frame=challenge requested=true allowed={allowed} main={:?}", action.target_is_main_frame); }
            if !allowed { eprintln!("liveness_probe site={site:?} stage={stage:?} navigation_refused=true"); }
            allowed
        })
        .with_navigation_event_handler(move |event| {
            eprintln!("liveness_probe site={site:?} stage={stage:?} frame=main phase={:?} elapsed_ms={}", event.phase, started.elapsed().as_millis());
            if let Some(gate) = &event_gate { let _ = gate.observe(event); }
        });
    if stage != LivenessStage::Bare {
        builder = builder.with_user_agent(super::safari_user_agent());
    }
    if hidden {
        builder = builder.with_background_throttling(wry::BackgroundThrottlingPolicy::Throttle);
    }
    if matches!(
        stage,
        LivenessStage::BrowseScripts | LivenessStage::DocumentGate | LivenessStage::Hidden
    ) {
        for (source, all_frames) in crate::host::protected_script_specs_for_native_probe() {
            builder = builder.with_initialization_script_for_main_only(source, !all_frames);
        }
    }
    let view = if owned_stage {
        use super::agent_context::{build_owned_work_view, AgentOwnedViewCallbacks};
        let store = super::new_ephemeral_data_store().map_err(|_| "store")?;
        let mut owned = build_owned_work_view(
            &host,
            zephium_agentic::ContextOwnedViewport::STANDARD,
            zephium_core::ids::ProfileId::generate(),
            zephium_agentic::ContextProfileStorageClass::Ephemeral,
            Some(&store),
            AgentOwnedViewCallbacks::new(
                |_| {},
                || {},
                || {},
                || eprintln!("liveness_probe invariant_failure=true"),
                || {},
            )
            .with_redirect(|_| eprintln!("liveness_probe redirect_refused=true")),
        )
        .map_err(|_| "owned_construction")?;
        let navigation = owned.work_navigation().ok_or("owned_gate")?.clone();
        bootstrap(&navigation)?;
        owned
            .prepare_semantic_document_load()
            .map_err(|_| "semantic_load")?;
        arm(&navigation, site, target)?;
        owned.view().load_url(target).map_err(|_| "owned_load")?;
        ProbePage::Owned(owned)
    } else {
        let view = builder.build_as_child(&host).map_err(|_| "construction")?;
        if let Some(gate) = &gate {
            bootstrap(gate)?;
            arm(gate, site, target)?;
            view.load_url(target).map_err(|_| "load")?;
        }
        ProbePage::Raw(view)
    };
    if super::native_webview(view.view()).frame().size != NSSize::new(1280.0, 800.0) {
        return Err("viewport");
    }
    if owned_stage {
        let page = super::native_webview(view.view());
        if matches!(
            stage,
            LivenessStage::OwnedUnthrottled
                | LivenessStage::OwnedOffscreenWindow
                | LivenessStage::OwnedOffscreenChild
        ) {
            // SAFETY: this release-excluded comparison owns the retained view on main.
            unsafe {
                page.configuration()
                    .preferences()
                    .setInactiveSchedulingPolicy(objc2_web_kit::WKInactiveSchedulingPolicy::None);
            }
        }
        if !hidden && stage != LivenessStage::OwnedHosted {
            view.view().set_visible(true).map_err(|_| "presentation")?;
        }
        if stage == LivenessStage::OwnedOffscreenWindow {
            window.setFrameOrigin(NSPoint::new(-8000.0, -8000.0));
            window.setIgnoresMouseEvents(true);
        }
        if stage == LivenessStage::OwnedOffscreenChild {
            let native: &NSView = &page;
            native.setFrameOrigin(NSPoint::new(-8000.0, -8000.0));
        }
    }
    if stage == LivenessStage::OwnedOffscreenWindow {
        window.orderFront(None);
    } else {
        window.makeKeyAndOrderFront(None);
        app.activate();
    }
    let mut presentation = if stage == LivenessStage::OwnedHosted {
        window.setIgnoresMouseEvents(true);
        let mut presentation = super::WorkObservationPresentation::prepare(
            view.view(),
            started + observation_window + SAMPLE_DEADLINE * 2,
            #[cfg(feature = "native-agentic-work-lifetime-diagnostic")]
            |_| {},
        )
        .map_err(|_| "hosted_prepare")?;
        let state = presentation.present();
        eprintln!("liveness_probe site={site:?} stage={stage:?} presentation={state:?}");
        Some(presentation)
    } else {
        None
    };
    while started.elapsed() < observation_window {
        pump();
    }
    if let Some(presentation) = &mut presentation {
        let state = presentation.poll();
        eprintln!("liveness_probe site={site:?} stage={stage:?} presentation={state:?}");
    }
    let gate_failed = match &view {
        ProbePage::Owned(owned) => owned.work_navigation().is_some_and(|gate| gate.failed()),
        ProbePage::Raw(_) => gate.as_ref().is_some_and(|gate| gate.failed()),
    };
    eprintln!("liveness_probe site={site:?} stage={stage:?} gate_failed={gate_failed}");
    if let Some(fixture) = &fixture {
        eprintln!(
            "liveness_probe site={site:?} stage={stage:?} animation_released={}",
            fixture.released()
        );
    }
    let page = super::native_webview(view.view());
    for sample in 0..3 {
        let facts = challenge_ax::inspect(&page);
        eprintln!("liveness_probe site={site:?} stage={stage:?} ax_sample={sample} nodes={} web_areas={} challenge_areas={} checkboxes={} scoped_checkboxes={} named_checkboxes={} enabled_checkboxes={} candidates={} unresolved_remote={} root_getters={} bridge={:?} truncated={}",
            facts.nodes, facts.web_areas, facts.challenge_areas, facts.checkboxes,
            facts.scoped_checkboxes, facts.named_checkboxes, facts.enabled_checkboxes, facts.candidates, facts.unresolved_remote, facts.root_getters, facts.bridge, facts.truncated);
        if facts.scoped_checkboxes > 0 && !facts.truncated {
            break;
        }
        let wait_started = Instant::now();
        while wait_started.elapsed() < Duration::from_millis(100) {
            pump();
        }
    }
    let facts = challenge_ax::inspect_own_process(mtm);
    eprintln!("liveness_probe site={site:?} stage={stage:?} ax_source=OwnProcess windows={} nodes={} web_areas={} challenge_areas={} checkboxes={} scoped_checkboxes={} named_checkboxes={} enabled_checkboxes={} candidates={} unresolved_remote={} bridge={:?} truncated={}",
        facts.windows,
        facts.nodes, facts.web_areas, facts.challenge_areas, facts.checkboxes,
        facts.scoped_checkboxes, facts.named_checkboxes, facts.enabled_checkboxes, facts.candidates, facts.unresolved_remote, facts.bridge, facts.truncated);
    let cookies = Rc::new(RefCell::new(None));
    let cookie_result = cookies.clone();
    let cookie_callback = RcBlock::new(move |values: NonNull<NSArray<NSHTTPCookie>>| {
        // SAFETY: WebKit retains the cookie array for this callback.
        let values = unsafe { values.as_ref() };
        let mut clearance = false;
        let mut bot_management = false;
        for cookie in values.iter().take(4096) {
            let name = cookie.name();
            clearance |= name.isEqualToString(objc2_foundation::ns_string!("cf_clearance"));
            bot_management |= name.isEqualToString(objc2_foundation::ns_string!("__cf_bm"));
        }
        cookie_result.replace(Some((values.len(), clearance, bot_management)));
    });
    // SAFETY: all retained WebKit objects are accessed on their main thread;
    // the copied callback reads names only, never cookie values.
    unsafe {
        page.configuration()
            .websiteDataStore()
            .httpCookieStore()
            .getAllCookies(&cookie_callback);
    }
    let cookie_start = Instant::now();
    while cookies.borrow().is_none() && cookie_start.elapsed() < SAMPLE_DEADLINE {
        pump();
    }
    if let Some((count, clearance, bot_management)) = cookies.borrow_mut().take() {
        eprintln!("liveness_probe site={site:?} stage={stage:?} cookies={count} clearance={clearance} bot_management={bot_management}");
    } else {
        eprintln!("liveness_probe site={site:?} stage={stage:?} cookie_sample_timeout=true");
    }
    let result = Rc::new(RefCell::new(None));
    let completed = result.clone();
    let callback = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let text = if error.is_null() {
            // SAFETY: WebKit supplies this object for the callback's duration.
            unsafe { value.as_ref() }
                .and_then(AnyObject::downcast_ref::<NSString>)
                .filter(|s| s.length() <= 2048)
                .map(ToString::to_string)
        } else {
            None
        };
        completed.replace(Some(text));
    });
    // SAFETY: a fixed read-only script, isolated world and retained main-thread page;
    // the callback owns its result state and WebKit copies the block.
    unsafe {
        let world =
            WKContentWorld::worldWithName(&NSString::from_str("zephium-liveness-probe"), mtm);
        page.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(SAMPLE),
            None,
            None,
            &world,
            Some(&callback),
        );
    }
    let sample_start = Instant::now();
    while result.borrow().is_none() && sample_start.elapsed() < SAMPLE_DEADLINE {
        pump();
    }
    drop(presentation);
    window.orderOut(None);
    let raw = result
        .borrow_mut()
        .take()
        .flatten()
        .ok_or("sample_timeout_or_error")?;
    let facts: serde_json::Value = serde_json::from_str(&raw).map_err(|_| "sample_schema")?;
    for field in ["complete", "visible", "challenge", "government_content"] {
        let value = facts
            .get(field)
            .and_then(serde_json::Value::as_bool)
            .ok_or("sample_schema")?;
        eprintln!("liveness_probe site={site:?} stage={stage:?} fact={field} value={value}");
    }
    for field in [
        "light_dom_challenge_frames",
        "light_dom_checkboxes",
        "raf",
        "timers",
        "timer_max_ms",
        "timer_mean_ms",
        "sample_ms",
        "load_ms",
        "body_characters",
    ] {
        let value = facts
            .get(field)
            .and_then(serde_json::Value::as_u64)
            .ok_or("sample_schema")?;
        eprintln!("liveness_probe site={site:?} stage={stage:?} fact={field} count={value}");
    }
    drop(view);
    window.close();
    Ok(())
}

fn bootstrap(
    gate: &crate::platform::work_document_navigation::WorkDocumentNavigation,
) -> Result<(), &'static str> {
    let start = Instant::now();
    while !gate.bootstrap_ready() && !gate.failed() && start.elapsed() < SAMPLE_DEADLINE {
        pump();
    }
    if gate.bootstrap_ready() {
        Ok(())
    } else {
        Err("bootstrap")
    }
}

fn arm(
    gate: &crate::platform::work_document_navigation::WorkDocumentNavigation,
    site: LivenessSite,
    target: &str,
) -> Result<(), &'static str> {
    gate.arm_with_policy(
        zephium_agentic::ContextNavigationTarget::parse(target).map_err(|_| "target")?,
        if site == LivenessSite::AnimationFixture {
            zephium_agentic::WorkBrowserDocumentPolicy::Exact
        } else if site == LivenessSite::GoogleResults {
            zephium_agentic::WorkBrowserDocumentPolicy::PublicResultsPage
        } else {
            zephium_agentic::WorkBrowserDocumentPolicy::PublicSameDocumentQuery
        },
    )
    .map_err(|_| "arm")
}
