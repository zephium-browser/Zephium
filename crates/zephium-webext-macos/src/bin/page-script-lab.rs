//! Checks the built-in page script in real WebKit: it runs once per document,
//! skips paused hosts, swaps programs without overlap and unloads.
//!
//! ```text
//! page-script-lab <scratch dir>
//! ```

use std::cell::RefCell;
use std::collections::BTreeSet;
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
use zephium_webext_macos::PageScript;

enum Step {
    Set(Option<&'static str>),
    SetTwice(&'static str, &'static str),
    Pause(&'static [&'static str]),
    Expect(&'static str),
}

const STEPS: &[Step] = &[
    Step::Set(Some("A")),
    Step::Expect("A:1"),
    Step::Pause(&["127.0.0.1"]),
    Step::Expect("undefined:undefined"),
    Step::Pause(&[]),
    Step::Expect("A:1"),
    Step::Set(Some("B")),
    Step::Expect("B:1"),
    Step::Set(Some("B")),
    Step::Expect("B:1"),
    Step::Pause(&["127.0.0.1"]),
    Step::Set(Some("A")),
    Step::Expect("undefined:undefined"),
    Step::Pause(&[]),
    Step::Set(None),
    Step::Expect("undefined:undefined"),
    Step::SetTwice("A", "B"),
    Step::Expect("B:1"),
];

struct Lab {
    mtm: MainThreadMarker,
    script: PageScript,
    view: Retained<WKWebView>,
    url: String,
    packages: PathBuf,
    failures: RefCell<usize>,
    _controller: Retained<WKWebExtensionController>,
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
    for name in ["A", "B"] {
        let dir = packages.join(name);
        if !dir.exists() {
            let program = format!(
                "globalThis.__ps = '{name}'; globalThis.__runs = (globalThis.__runs || 0) + 1;"
            );
            PageScript::write(&dir, &program).expect("package written");
        }
    }

    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = [0u8; 4096];
            let _ = stream.read(&mut buffer);
            let body = "<!doctype html><title>lab</title><p>page</p>";
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });

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
        url,
        packages,
        failures: RefCell::new(0),
        _controller: controller,
    });
    let app = NSApplication::sharedApplication(mtm);
    run(lab, 0);
    app.run();
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
        Step::Set(name) => {
            let dir = name.map(|name| lab.packages.join(name));
            println!("set {name:?}");
            lab.script.set(dir.as_deref(), move |result| {
                if let Err(error) = result {
                    println!("  load failed: {error}");
                }
                next();
            });
        }
        Step::SetTwice(first, second) => {
            println!("set {first} then {second} at once");
            lab.script.set(Some(&lab.packages.join(first)), |_| {});
            lab.script
                .set(Some(&lab.packages.join(second)), move |_| next());
        }
        Step::Pause(hosts) => {
            println!("pause {hosts:?}");
            lab.script.set_paused(
                hosts
                    .iter()
                    .map(|host| host.to_string())
                    .collect::<BTreeSet<_>>(),
            );
            next();
        }
        Step::Expect(expected) => {
            let url = NSURL::URLWithString(&NSString::from_str(&lab.url)).unwrap();
            unsafe { lab.view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
            let view = lab.view.clone();
            let lab = lab.clone();
            when_loaded(view, move || {
                let probe = "return String(globalThis.__ps) + ':' + String(globalThis.__runs)";
                let checked = lab.clone();
                evaluate(lab.mtm, &lab.view, probe, move |value| {
                    let ok = value == *expected;
                    println!(
                        "  {} {value} (want {expected})",
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
