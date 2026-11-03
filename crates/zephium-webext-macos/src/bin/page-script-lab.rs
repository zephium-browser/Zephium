//! Checks the built-in page script in real WebKit: it runs once per document
//! in the page and its frames, skips every frame of a page on a paused site
//! but runs in a paused site's frame elsewhere, swaps programs without
//! overlap and unloads.
//!
//! ```text
//! page-script-lab <scratch dir>
//! ```

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSError, NSRect, NSString, NSTimer, NSURLRequest, NSURL};
use objc2_web_kit::{
    WKContentWorld, WKWebExtensionController, WKWebExtensionControllerConfiguration, WKWebView,
    WKWebViewConfiguration,
};
use zephium_core::blocker::{scriptlet_program_with_pauses, BlockerSite};
use zephium_webext_macos::PageScript;

const PAUSED: &str = "127.0.0.1";
const OTHER: &str = "localhost";
const NOTHING: &str = "undefined:undefined|undefined:undefined";

enum Step {
    /// A program and the sites paused in it, or nothing.
    Set(Option<(&'static str, &'static [&'static str])>),
    SetTwice(&'static str, &'static str),
    /// Opens a page on the first host framing a page on the second, and
    /// wants `top|frame`, each as `program:runs`.
    Expect(&'static str, &'static str, &'static str),
}

const STEPS: &[Step] = &[
    Step::Set(Some(("A", &[]))),
    Step::Expect(PAUSED, OTHER, "A:1|A:1"),
    Step::Set(Some(("A", &[PAUSED]))),
    Step::Expect(PAUSED, OTHER, NOTHING),
    Step::Expect(OTHER, PAUSED, "A:1|A:1"),
    Step::Set(Some(("A", &[]))),
    Step::Expect(PAUSED, OTHER, "A:1|A:1"),
    Step::Set(Some(("B", &[]))),
    Step::Expect(PAUSED, OTHER, "B:1|B:1"),
    Step::Set(Some(("B", &[]))),
    Step::Expect(OTHER, PAUSED, "B:1|B:1"),
    Step::Set(None),
    Step::Expect(PAUSED, OTHER, NOTHING),
    Step::SetTwice("A", "B"),
    Step::Expect(PAUSED, OTHER, "B:1|B:1"),
];

struct Lab {
    mtm: MainThreadMarker,
    script: PageScript,
    view: Retained<WKWebView>,
    port: u16,
    packages: PathBuf,
    failures: RefCell<usize>,
    _controller: Retained<WKWebExtensionController>,
}

impl Lab {
    fn package(&self, name: &str, paused: &[&str]) -> PathBuf {
        let dir = self.packages.join(format!("{name}-{}", paused.join("+")));
        if !dir.exists() {
            let program = format!(
                "(() => {{ globalThis.__ps = '{name}'; globalThis.__runs = (globalThis.__runs || 0) + 1; }})();\n"
            );
            let sites: Vec<BlockerSite> = paused
                .iter()
                .map(|host| BlockerSite::try_from(host.to_string()).expect("a site"))
                .collect();
            let program = scriptlet_program_with_pauses(&program, &sites);
            PageScript::write(&dir, &program).expect("package written");
        }
        dir
    }
}

fn main() {
    let mtm = MainThreadMarker::new().expect("runs on the main thread");
    let scratch = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: page-script-lab <dir>"),
    );
    if !PageScript::supported() {
        println!("SKIP page scripts need macOS 15.4");
        return;
    }
    let packages = scratch.join("packages");
    std::fs::create_dir_all(&packages).expect("scratch directory");

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let port = listener.local_addr().unwrap().port();
    // `localhost` resolves to ::1 first; without a listener there, loads stall.
    if let Ok(v6) = TcpListener::bind(("::1", port)) {
        std::thread::spawn(move || serve(v6));
    }
    std::thread::spawn(move || serve(listener));

    let controller = unsafe {
        WKWebExtensionController::initWithConfiguration(
            WKWebExtensionController::alloc(mtm),
            &WKWebExtensionControllerConfiguration::nonPersistentConfiguration(mtm),
        )
    };
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { configuration.setWebExtensionController(Some(&controller)) };
    let view = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::default(),
            &configuration,
        )
    };
    let lab = Rc::new(Lab {
        mtm,
        script: PageScript::new(&controller, false),
        view,
        port,
        packages,
        failures: RefCell::new(0),
        _controller: controller,
    });
    let app = NSApplication::sharedApplication(mtm);
    run(lab, 0);
    app.run();
}

// WebKit opens connections it may never use, so each gets its own thread.
fn serve(listener: TcpListener) {
    for stream in listener.incoming().flatten() {
        std::thread::spawn(move || answer(stream));
    }
}

fn answer(mut stream: std::net::TcpStream) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let mut buffer = [0u8; 4096];
    let read = stream.read(&mut buffer).unwrap_or(0);
    let request = String::from_utf8_lossy(&buffer[..read]);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let body = match path.strip_prefix("/top?frame=") {
        Some(frame) => format!(
            "<!doctype html><script>addEventListener('message', (e) => {{ window.__frame = e.data; }});</script><iframe src=\"{frame}\"></iframe>"
        ),
        None => "<!doctype html><script>parent.postMessage(String(globalThis.__ps) + ':' + String(globalThis.__runs), '*');</script>".to_owned(),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

fn run(lab: Rc<Lab>, index: usize) {
    let Some(step) = STEPS.get(index) else {
        let failures = *lab.failures.borrow();
        println!("{}", if failures == 0 { "ALL OK" } else { "FAILED" });
        std::process::exit(i32::from(failures != 0));
    };
    let next = {
        let lab = lab.clone();
        move || run(lab, index + 1)
    };
    match step {
        Step::Set(program) => {
            let dir = program.map(|(name, paused)| lab.package(name, paused));
            println!("set {program:?}");
            lab.script.set(dir.as_deref(), move |result| {
                if let Err(error) = result {
                    println!("  load failed: {error}");
                }
                next();
            });
        }
        Step::SetTwice(first, second) => {
            println!("set {first} then {second} at once");
            lab.script.set(Some(&lab.package(first, &[])), |_| {});
            lab.script
                .set(Some(&lab.package(second, &[])), move |_| next());
        }
        Step::Expect(top, frame, expected) => {
            let port = lab.port;
            let url = format!("http://{top}:{port}/top?frame=http://{frame}:{port}/leaf");
            let url = NSURL::URLWithString(&NSString::from_str(&url)).unwrap();
            unsafe { lab.view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            let view = lab.view.clone();
            let lab = lab.clone();
            when_loaded(view, move || {
                let probe = "for (let i = 0; i < 40 && window.__frame === undefined; i++) await new Promise((r) => setTimeout(r, 50));\n\
                             return String(globalThis.__ps) + ':' + String(globalThis.__runs) + '|' + String(window.__frame);";
                let checked = lab.clone();
                evaluate(lab.mtm, &lab.view, probe, move |value| {
                    let ok = value == *expected;
                    println!(
                        "  {top} framing {frame}: {} {value} (want {expected})",
                        if ok { "ok" } else { "FAIL" }
                    );
                    if !ok {
                        *checked.failures.borrow_mut() += 1;
                    }
                    next();
                });
            });
        }
    }
}

fn when_loaded(view: Retained<WKWebView>, next: impl FnOnce() + 'static) {
    let next = RefCell::new(Some(next));
    let started = std::time::Instant::now();
    let block = RcBlock::new(move |timer: NonNull<NSTimer>| {
        let loaded = !unsafe { view.isLoading() } && unsafe { view.URL() }.is_some();
        let late = started.elapsed().as_secs() > 15;
        if loaded || late {
            unsafe { timer.as_ref().invalidate() };
            if late {
                println!("  load timed out");
            }
            if let Some(next) = next.take() {
                next();
            }
        }
    });
    unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &block) };
}

fn evaluate(
    mtm: MainThreadMarker,
    view: &WKWebView,
    body: &str,
    done: impl FnOnce(String) + 'static,
) {
    let done = RefCell::new(Some(done));
    let block = RcBlock::new(move |value: *mut AnyObject, error: *mut NSError| {
        let Some(done) = done.take() else { return };
        if let Some(error) = unsafe { error.as_ref() } {
            return done(format!(
                "error {}",
                zephium_webext_macos::describe_error(error)
            ));
        }
        let text = unsafe { value.as_ref() }
            .and_then(|value| value.downcast_ref::<NSString>())
            .map(|s| s.to_string())
            .unwrap_or_else(|| "none".into());
        done(text);
    });
    unsafe {
        view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(body),
            None,
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&block),
        )
    };
}
