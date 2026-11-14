use std::{
    cell::RefCell,
    collections::HashMap,
    rc::{Rc, Weak},
    time::Duration,
};

use gtk::{gio, glib, prelude::*};
use webkit2gtk::{LoadEvent, WebViewExt};
use wry::WebViewExtUnix;

use super::MAX_RESULT_BYTES;

// A favicon watch has its own 30-second hard limit inside the document.
const FAVICON_TIMEOUT: Duration = Duration::from_secs(32);
const FAVICON_RESULT_UNITS: usize = (MAX_RESULT_BYTES - 1) / 3;
// Generic batches allow 128 KiB of JSON. A primitive UTF-16 string of this
// length needs at most 384 KiB of UTF-8 before the caller's stricter parser.
const GENERIC_RESULT_UNITS: usize = 128 * 1024;
type Completion = Box<dyn FnOnce(Option<String>) + Send>;

thread_local! {
    static PENDING: RefCell<HashMap<usize, Vec<Weak<Evaluation>>>> = RefCell::new(HashMap::new());
}

struct Evaluation {
    page_key: usize,
    page: glib::WeakRef<webkit2gtk::WebView>,
    done: RefCell<Option<Completion>>,
    cancellable: gio::Cancellable,
    timeout: RefCell<Option<glib::SourceId>>,
    signals: RefCell<Vec<glib::SignalHandlerId>>,
}

impl Evaluation {
    fn finish(&self, result: Option<String>) {
        let Some(done) = self.done.borrow_mut().take() else {
            return;
        };
        if let Some(timeout) = self.timeout.borrow_mut().take() {
            timeout.remove();
        }
        let signals = self.signals.take();
        if let Some(page) = self.page.upgrade() {
            for signal in signals {
                page.disconnect(signal);
            }
        }
        PENDING.with(|pending| {
            let mut pending = pending.borrow_mut();
            if let Some(operations) = pending.get_mut(&self.page_key) {
                operations.retain(|operation| !std::ptr::eq(operation.as_ptr(), self));
                if operations.is_empty() {
                    pending.remove(&self.page_key);
                }
            }
        });
        // Take the completion before cancellation, which can itself dispatch
        // a native callback. Only WebKit's callback releases its native result.
        self.cancellable.cancel();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| done(result)));
    }
}

pub(crate) fn evaluate(
    view: &wry::WebView,
    expression: &str,
    done: impl FnOnce(Option<String>) + Send + 'static,
) -> bool {
    evaluate_page(
        &view.webview(),
        expression,
        FAVICON_RESULT_UNITS,
        Some(FAVICON_TIMEOUT),
        Box::new(done),
    )
}

pub(crate) fn evaluate_until_navigation(
    view: &wry::WebView,
    expression: &str,
    done: impl FnOnce(Option<String>) + Send + 'static,
) -> bool {
    evaluate_page(
        &view.webview(),
        expression,
        GENERIC_RESULT_UNITS,
        None,
        Box::new(done),
    )
}

pub(crate) fn cancel(view: &wry::WebView) {
    cancel_page(&view.webview());
}

fn cancel_page(page: &webkit2gtk::WebView) {
    let pending = PENDING.with(|pending| pending.borrow_mut().remove(&(page.as_ptr() as usize)));
    for operation in pending
        .into_iter()
        .flatten()
        .filter_map(|operation| operation.upgrade())
    {
        operation.finish(None);
    }
}

fn evaluate_page(
    page: &webkit2gtk::WebView,
    expression: &str,
    max_units: usize,
    deadline: Option<Duration>,
    done: Completion,
) -> bool {
    if !gtk::is_initialized_main_thread() {
        return false;
    }
    let pending = Rc::new(Evaluation {
        page_key: page.as_ptr() as usize,
        page: page.downgrade(),
        done: RefCell::new(Some(done)),
        cancellable: gio::Cancellable::new(),
        timeout: RefCell::new(None),
        signals: RefCell::new(Vec::new()),
    });
    PENDING.with(|operations| {
        operations
            .borrow_mut()
            .entry(page.as_ptr() as usize)
            .or_default()
            .push(Rc::downgrade(&pending));
    });
    let navigating = pending.clone();
    let navigation = page.connect_load_changed(move |_, event| {
        if event == LoadEvent::Started {
            navigating.finish(None);
        }
    });
    let destroying = pending.clone();
    let destroy = page.connect_destroy(move |_| destroying.finish(None));
    let terminated = pending.clone();
    let process = page.connect_web_process_terminated(move |_, _| terminated.finish(None));
    *pending.signals.borrow_mut() = vec![navigation, destroy, process];
    if let Some(deadline) = deadline {
        let timed_out = pending.clone();
        *pending.timeout.borrow_mut() = Some(glib::timeout_add_local_once(deadline, move || {
            // This source removes itself after returning.
            timed_out.timeout.borrow_mut().take();
            timed_out.finish(None);
        }));
    }
    let cancellable = pending.cancellable.clone();
    page.call_async_javascript_function(
        &bounded_body(expression, max_units),
        None,
        None,
        None,
        Some(&cancellable),
        move |result| {
            if pending.done.borrow().is_none() {
                return;
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                result.ok().and_then(|value| {
                    // The host body always returns a primitive string. Its
                    // bound is enforced before WebKit serializes it to JSC.
                    decode_result(&value.to_string(), max_units)
                })
            }))
            .ok()
            .flatten();
            pending.finish(result);
        },
    );
    true
}

fn bounded_body(expression: &str, max_units: usize) -> String {
    // Primitive length/index access cannot be replaced by the page. NUL and
    // backslash are escaped so JSC's C-string conversion preserves both.
    // Every unit costs at most three UTF-8 bytes, including either escape.
    format!(
        r#"const value = await ({expression});
if (typeof value !== 'string' || value.length > {max_units}) return '';
let result = 's';
for (let index = 0; index < value.length; index++) {{
    const unit = value[index];
    result += unit === '\0' ? '\\0' : unit === '\\' ? '\\\\' : unit;
}}
return result;"#
    )
}

fn decode_result(result: &str, max_units: usize) -> Option<String> {
    if result.len() > max_units.checked_mul(3)?.checked_add(1)? {
        return None;
    }
    let mut characters = result.strip_prefix('s')?.chars();
    let mut decoded = String::with_capacity(result.len() - 1);
    while let Some(character) = characters.next() {
        decoded.push(if character == '\\' {
            match characters.next()? {
                '0' => '\0',
                '\\' => '\\',
                _ => return None,
            }
        } else {
            character
        });
    }
    (decoded.len() <= MAX_RESULT_BYTES).then_some(decoded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::Instant};
    use webkit2gtk::WebContextExt;

    #[test]
    fn the_primitive_envelope_preserves_empty_nul_and_escape_sequences() {
        assert_eq!(decode_result("s", 10).as_deref(), Some(""));
        assert_eq!(decode_result("sa\\0b\\\\c", 10).as_deref(), Some("a\0b\\c"));
        for malformed in ["", "null", "s\\", "s\\x"] {
            assert_eq!(decode_result(malformed, 10), None);
        }
        assert_eq!(decode_result(&format!("s{}", "x".repeat(31)), 10), None);
    }

    fn begin(
        page: &webkit2gtk::WebView,
        expression: &str,
        deadline: Option<Duration>,
    ) -> mpsc::Receiver<Option<String>> {
        let (sender, receiver) = mpsc::channel();
        assert!(evaluate_page(
            page,
            expression,
            FAVICON_RESULT_UNITS,
            deadline,
            Box::new(move |result| sender.send(result).unwrap()),
        ));
        receiver
    }

    fn receive(receiver: &mpsc::Receiver<Option<String>>) -> Option<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            match receiver.try_recv() {
                Ok(result) => return result,
                Err(mpsc::TryRecvError::Empty) => {}
                Err(error) => panic!("native evaluation channel failed: {error}"),
            }
            assert!(
                Instant::now() < deadline,
                "native evaluation did not settle"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn load(page: &webkit2gtk::WebView, title: &str) {
        page.load_html(&format!("<!doctype html><title>{title}</title>"), None);
        let deadline = Instant::now() + Duration::from_secs(5);
        while page.title().as_deref() != Some(title) {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            assert!(Instant::now() < deadline, "native fixture did not load");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    #[ignore = "requires an isolated display and admitted sandboxed WebKitGTK"]
    fn native_async_pulls_are_bounded_and_cancelled_exactly_once() {
        crate::platform::linux::enforce_runtime_security_floor().expect("admitted WebKitGTK");
        gtk::init().expect("isolated GTK display");
        let context = webkit2gtk::WebContext::new_ephemeral();
        context.set_sandbox_enabled(true);
        let page = webkit2gtk::WebView::with_context(&context);
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.add(&page);
        window.show_all();
        load(&page, "initial");

        for (expression, expected) in [
            ("(async () => 'ready')()", "ready"),
            ("''", ""),
            ("'a\\0b\\\\c'", "a\0b\\c"),
            ("'😀'", "😀"),
        ] {
            assert_eq!(
                receive(&begin(&page, expression, None)).as_deref(),
                Some(expected)
            );
        }
        for expression in [
            "(async () => { throw new Error('rejected'); })()",
            "null",
            "undefined",
            "42",
            "({ length: 1, toString() { return 'hostile'; } })",
        ] {
            assert_eq!(receive(&begin(&page, expression, None)), None);
        }
        let oversize = format!("'x'.repeat({})", FAVICON_RESULT_UNITS + 1);
        assert_eq!(receive(&begin(&page, &oversize, None)), None);
        let unicode = format!("'\\u20ac'.repeat({FAVICON_RESULT_UNITS})");
        let unicode = receive(&begin(&page, &unicode, None)).unwrap();
        assert_eq!(unicode.len(), FAVICON_RESULT_UNITS * 3);
        let astral = format!("'😀'.repeat({})", FAVICON_RESULT_UNITS / 2);
        let astral = receive(&begin(&page, &astral, None)).unwrap();
        assert_eq!(astral, "😀".repeat(FAVICON_RESULT_UNITS / 2));
        let oversize_astral = format!("'😀'.repeat({})", FAVICON_RESULT_UNITS / 2 + 1);
        assert_eq!(receive(&begin(&page, &oversize_astral, None)), None);

        let (sender, receiver) = mpsc::channel();
        let reenter: glib::SendWeakRef<webkit2gtk::WebView> = page.downgrade().into();
        assert!(evaluate_page(
            &page,
            "'first'",
            FAVICON_RESULT_UNITS,
            None,
            Box::new(move |result| {
                assert_eq!(result.as_deref(), Some("first"));
                let page = reenter.upgrade().unwrap();
                assert!(evaluate_page(
                    &page,
                    "'reentered'",
                    FAVICON_RESULT_UNITS,
                    None,
                    Box::new(move |result| sender.send(result).unwrap()),
                ));
            }),
        ));
        assert_eq!(receive(&receiver).as_deref(), Some("reentered"));

        let stalled = begin(
            &page,
            "new Promise(() => {})",
            Some(Duration::from_millis(20)),
        );
        assert_eq!(receive(&stalled), None);
        assert!(matches!(
            stalled.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));

        let late = begin(
            &page,
            "new Promise(resolve => setTimeout(() => { globalThis.late = true; resolve('late'); }, 80))",
            Some(Duration::from_millis(20)),
        );
        assert_eq!(receive(&late), None);
        let wait = Instant::now() + Duration::from_millis(120);
        while Instant::now() < wait {
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            receive(&begin(&page, "late ? 'finished' : ''", None)).as_deref(),
            Some("finished")
        );
        assert!(matches!(
            late.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));

        let navigating = begin(&page, "new Promise(() => {})", None);
        load(&page, "next");
        assert_eq!(receive(&navigating), None);
        let retired = begin(&page, "new Promise(() => {})", None);
        cancel_page(&page);
        assert_eq!(receive(&retired), None);

        let overrides = r#"(() => {
            String.prototype.replace = String.prototype.charCodeAt = () => { throw 1; };
            globalThis.String = globalThis.Promise = globalThis.TextEncoder = () => { throw 2; };
            globalThis.JSON = { stringify() { throw 3; } };
            return 'safe\0\\value';
        })()"#;
        assert_eq!(
            receive(&begin(&page, overrides, None)).as_deref(),
            Some("safe\0\\value")
        );
        load(&page, "destroy");
        let destroyed = begin(&page, "new Promise(() => {})", None);
        window.close();
        assert_eq!(receive(&destroyed), None);
        assert!(PENDING.with(|pending| pending.borrow().is_empty()));
    }
}
