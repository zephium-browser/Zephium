//! Explicit native qualification using only disposable owned WebView2 profiles.
//! Not compiled into the browser. Results and packages remain local.
use super::*;
use base64::Engine as _;
use raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};
use std::num::NonZeroIsize;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use wry::{WebView, WebViewBuilder};
use zephium_core::blocker::{
    BlockerSite, ContentPolicyGeneration, ContentRuleDigest, ContentRules, PreparedBlockerSites,
};
use zephium_core::geometry::Rect;
use zephium_core::ports::engine::{Partition, UserContent, UserContentGeneration};
type Task = Box<dyn FnOnce() + Send>;
thread_local! { static TASKS: RefCell<Option<mpsc::Receiver<Task>>> = const { RefCell::new(None) }; }
thread_local! { static TOKENS: RefCell<Vec<Arc<AtomicBool>>> = const { RefCell::new(Vec::new()) }; }
fn live_token() -> Arc<AtomicBool> {
    let token = Arc::new(AtomicBool::new(true));
    TOKENS.with(|tokens| tokens.borrow_mut().push(token.clone()));
    token
}
struct Host(HWND);
impl Host {
    fn new() -> Self {
        // SAFETY: process-lifetime class and UI-apartment window procedure.
        unsafe {
            let module = GetModuleHandleW(None).unwrap();
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: module.into(),
                lpszClassName: windows_core::w!("ZephiumRuntimeQualification"),
                ..Default::default()
            };
            RegisterClassW(&class);
            Self(
                CreateWindowExW(
                    WS_EX_TOOLWINDOW,
                    class.lpszClassName,
                    windows_core::w!("Zephium Protection Qualification"),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    1200,
                    800,
                    None,
                    None,
                    Some(module.into()),
                    None,
                )
                .unwrap(),
            )
        }
    }
}
impl HasWindowHandle for Host {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = NonZeroIsize::new(self.0 .0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: borrowed handle cannot outlive this owner.
        Ok(
            unsafe {
                WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(raw)))
            },
        )
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = unsafe { DestroyWindow(self.0) };
    }
}
unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wp, lp) }
}
fn pump() {
    TASKS.with(|tasks| {
        while let Some(task) = tasks.borrow().as_ref().and_then(|rx| rx.try_recv().ok()) {
            task();
        }
    });
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        // Bounded load/evaluation wait for this opt-in qualification.
        MsgWaitForMultipleObjectsEx(None, 20, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
    }
}
#[track_caller]
fn until(mut condition: impl FnMut() -> bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while !condition() {
        assert!(Instant::now() < deadline, "native qualification timed out");
        pump();
    }
}
fn evaluate(view: &WebView, script: &str) -> Value {
    let (tx, rx) = mpsc::channel();
    view.evaluate_script_with_callback(script, move |value| {
        let _ = tx.send(serde_json::from_str(&value).unwrap_or(Value::Null));
    })
    .unwrap();
    let mut result = None;
    until(
        || {
            result = rx.try_recv().ok();
            result.is_some()
        },
        10,
    );
    result.unwrap()
}

fn host<R: 'static>(f: impl FnOnce(&mut super::super::EngineHost) -> R + 'static) -> R {
    let result = Rc::new(RefCell::new(None));
    let out = result.clone();
    assert!(super::super::dispatch::try_with(move |host| {
        *out.borrow_mut() = Some(f(host));
    }));
    let value = result
        .borrow_mut()
        .take()
        .expect("synchronous test host access");
    value
}
fn crash(view: &WebView) {
    let callback =
        webview2_com::CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
    // Only this disposable controller. No remote-debugging endpoint is enabled.
    unsafe {
        view.webview()
            .CallDevToolsProtocolMethod(
                &windows::core::HSTRING::from("Page.crash"),
                &windows::core::HSTRING::from("{}"),
                &callback,
            )
            .unwrap();
    }
}
fn package(root: &Path, index: u128) -> WebExtensionLoad {
    let source =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../zephium-webext-windows/fixtures/storage");
    let dir = root.join(format!("package-{index}"));
    zephium_webext::archive::copy_dir(&source, &dir, &Default::default()).unwrap();
    let path = dir.join("manifest.json");
    let mut manifest: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut key = base64::engine::general_purpose::STANDARD
        .decode(manifest["key"].as_str().unwrap())
        .unwrap();
    // Distinct valid SPKI-shaped public test identities; no private keys or signatures.
    key[40] = index as u8;
    manifest["key"] = base64::engine::general_purpose::STANDARD
        .encode(&key)
        .into();
    std::fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    zephium_webext::windows::prepare(&dir, None).unwrap();
    WebExtensionLoad {
        install: ExtensionInstallId::from(index),
        extension_id: zephium_webext::ExtensionId::from_public_key(&key).to_string(),
        root: dir,
        permissions: vec![],
        match_patterns: vec![],
        start_background: false,
    }
}

#[test]
#[ignore = "requires an interactive Windows desktop and installed WebView2; disposable native runtime qualification"]
fn native_windows_runtime_recovery() {
    unsafe {
        windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        )
        .ok()
        .unwrap();
    }
    let window = Host::new();
    let restart = std::env::var_os("ZEPHIUM_QUALIFICATION_RESTART_ROOT");
    let root = if let Some(path) = &restart {
        let path = PathBuf::from(path).canonicalize().unwrap();
        assert_eq!(
            path.parent().unwrap(),
            std::env::temp_dir().canonicalize().unwrap()
        );
        assert!(path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("zephium-runtime-"));
        path
    } else {
        std::env::temp_dir().join(format!("zephium-runtime-{}", ProfileId::generate()))
    };
    std::fs::create_dir_all(&root).unwrap();
    let (tx, rx) = mpsc::channel::<Box<dyn FnOnce() + Send>>();
    TASKS.with(|slot| *slot.borrow_mut() = Some(rx));
    let dispatch: crate::MainThreadDispatch = Arc::new(move |task| tx.send(task).is_ok());
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = events.clone();
    super::super::dispatch::install(
        window.window_handle().unwrap().as_raw(),
        root.join("engine"),
        dispatch.clone(),
        UserContentGeneration::new(1).unwrap(),
        UserContent::default(),
        Arc::new(crate::NativeOpenAuthority {
            retirement: Default::default(),
            dispatch,
            fatal: Arc::new(|reason| panic!("{reason}")),
        }),
        Arc::new(move |event| recorded.lock().unwrap().push(event.event)),
        Arc::new(|reason| panic!("{reason}")),
    )
    .unwrap();
    struct Shutdown(bool);
    impl Shutdown {
        fn finish(&mut self) -> bool {
            if self.0 {
                return true;
            }
            self.0 = true;
            let (tx, rx) = mpsc::channel();
            super::super::dispatch::shutdown(Box::new(move |clean| {
                let _ = tx.send(clean);
            }));
            let deadline = Instant::now() + Duration::from_secs(8);
            let clean = loop {
                if let Ok(clean) = rx.try_recv() {
                    break clean;
                }
                if Instant::now() >= deadline {
                    break false;
                }
                pump();
            };
            // Drop apartment-bound observers while the test thread's TLS and
            // Win32 host are still alive, after the production shutdown barrier.
            super::super::dispatch::make_unavailable_for_test();
            clean
        }
    }
    impl Drop for Shutdown {
        fn drop(&mut self) {
            self.finish();
        }
    }
    let mut shutdown = Shutdown(false);
    let profile = if restart.is_some() {
        ProfileId::parse(&std::env::var("ZEPHIUM_QUALIFICATION_RESTART_PROFILE").unwrap()).unwrap()
    } else {
        ProfileId::generate()
    };
    let load = if restart.is_some() {
        let dir = root.join("package-1");
        let manifest: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        let key = base64::engine::general_purpose::STANDARD
            .decode(manifest["key"].as_str().unwrap())
            .unwrap();
        WebExtensionLoad {
            install: ExtensionInstallId::from(1),
            extension_id: zephium_webext::ExtensionId::from_public_key(&key).to_string(),
            root: dir,
            permissions: vec![],
            match_patterns: vec![],
            start_background: false,
        }
    } else {
        package(&root, 1)
    };
    let first = load.clone();
    host(move |h| h.load_windows_extension(profile, first)).unwrap();
    let key = (profile, load.install);
    if restart.is_some() {
        let tab = ItemId::from(200);
        let target = format!("chrome-extension://{}/popup.html", load.extension_id);
        host(move |h| {
            h.install_content_rules(
                profile,
                ContentPolicyGeneration::new(1).unwrap(),
                ContentRules::allow_all(ContentRuleDigest::from_bytes([0; 32])),
            );
            h.create_view(
                tab,
                Partition::Persistent(profile),
                &target,
                Rect::new(0.0, 0.0, 600.0, 400.0),
                live_token(),
            );
        });
        until(
            || host(move |h| h.views[&tab].navigation.committed_snapshot().is_some()),
            15,
        );
        host(move |h| {
            evaluate(&h.views[&tab], "window.close();true");
        });
        until(
            || {
                events.lock().unwrap().iter().any(|event| matches!(event, EngineEvent::NativeTabCloseRequested {id} if *id == tab))
            },
            10,
        );
        host(move |h| h.close(tab));
        assert!(shutdown.finish());
        println!(
            "RUNTIME_PASS persisted extension page closed after a complete native process restart"
        );
        return;
    }

    until(
        || {
            host(move |h| {
                evaluate(
                    &h.windows_extensions.installs[&key].bridge.view,
                    "typeof window.__zephiumRefresh",
                ) == "function"
            })
        },
        15,
    );
    host(move |h| {
        evaluate(
            &h.windows_extensions.installs[&key].bridge.view,
            "globalThis.preCrash = true",
        );
        crash(&h.windows_extensions.installs[&key].bridge.view);
    });
    until(
        || {
            host(move |h| {
                h.windows_extensions.installs[&key]
                    .action
                    .borrow()
                    .recovery
                    .attempts
                    > 0
            })
        },
        10,
    );
    until(
        || {
            host(move |h| {
                !h.windows_extensions.installs[&key]
                    .action
                    .borrow()
                    .recovery
                    .pending
            })
        },
        10,
    );
    until(
        || {
            host(move |h| {
                evaluate(
                    &h.windows_extensions.installs[&key].bridge.view,
                    "typeof window.__zephiumRefresh === 'function' && typeof preCrash === 'undefined'",
                ) == true
            })
        },
        15,
    );
    println!("RUNTIME_PASS host renderer crash reloaded host.html");

    let tab = ItemId::from(100);
    let target = format!("chrome-extension://{}/popup.html", load.extension_id);
    host(move |h| {
        h.install_content_rules(
            profile,
            ContentPolicyGeneration::new(1).unwrap(),
            ContentRules::allow_all(ContentRuleDigest::from_bytes([0; 32])),
        );
        h.create_view(
            tab,
            Partition::Persistent(profile),
            &target,
            Rect::new(0.0, 0.0, 600.0, 400.0),
            live_token(),
        );
        assert!(h.views.contains_key(&tab));
    });
    until(
        || host(move |h| h.views[&tab].navigation.committed_snapshot().is_some()),
        15,
    );
    println!("RUNTIME_STEP extension document committed");
    let second = ItemId::from(101);
    host(move |h| {
        h.create_view(
            second,
            Partition::Persistent(profile),
            "about:blank",
            Rect::new(0.0, 0.0, 600.0, 400.0),
            live_token(),
        )
    });
    let windows = host(move |h| {
        [
            native::window_id(&h.views[&tab].webview()).unwrap(),
            native::window_id(&h.views[&second].webview()).unwrap(),
        ]
    });
    host(move |h| {
        evaluate(
            &h.windows_extensions.installs[&key].bridge.view,
            "globalThis.qaSnapshots=0;globalThis.qaReports=0;globalThis.qaError=null;const send=chrome.runtime.sendMessage;chrome.runtime.sendMessage=function(...args){if(args[0]?.__zephiumActionSnapshot)qaSnapshots++;return Reflect.apply(send,this,args)};const post=chrome.webview.postMessage.bind(chrome.webview);chrome.webview.postMessage=function(message){if(JSON.parse(message).kind==='action')qaReports++;else qaError=message;post(message)};true",
        );
    });
    println!("RUNTIME_STEP native window identities {windows:?}");
    for index in 0..20 {
        let window = windows[index % 2];
        host(move |h| {
            h.windows_extensions.installs[&key]
                .bridge
                .view
                .evaluate_script(&format!("window.__zephiumRefresh({window})"))
                .unwrap();
        });
        until(
            || {
                host(move |h| {
                    let error =
                        evaluate(&h.windows_extensions.installs[&key].bridge.view, "qaError");
                    assert!(error.is_null(), "action error: {error}");
                    evaluate(
                        &h.windows_extensions.installs[&key].bridge.view,
                        "qaReports",
                    )
                    .as_u64()
                    .unwrap_or(0)
                        > index as u64
                })
            },
            10,
        );
    }
    let snapshots = host(move |h| {
        evaluate(
            &h.windows_extensions.installs[&key].bridge.view,
            "qaSnapshots",
        )
    })
    .as_u64()
    .unwrap();
    assert!(
        (1..=2).contains(&snapshots),
        "unbounded worker snapshots: {snapshots}"
    );
    println!(
        "RUNTIME_PASS missing icon: twenty native tab switches caused {snapshots} bounded startup snapshots"
    );
    // Keep native RPC and decoding paths; inject only a broken icon report.
    host(move |h| {
        evaluate(
            &h.windows_extensions.installs[&key].bridge.view,
            "qaSnapshots=0;qaReports=0;globalThis.qaNotification=null;chrome.runtime.onMessage.addListener((message,sender)=>{qaNotification={message,senderId:sender.id}});const sendBroken=chrome.runtime.sendMessage;chrome.runtime.sendMessage=function(...args){const result=Reflect.apply(sendBroken,this,args);return args[0]?.__zephiumActionSnapshot?result.catch(()=>null).then(()=>({icon:{path:'missing-qualification.png'},perTabIcons:false})):result};true",
        );
        assert_eq!(evaluate(
            &h.views[&tab],
            "(()=>{try{globalThis.qaSignal='pending';chrome.runtime.sendMessage({__zephiumActionChanged:true}).then(()=>qaSignal='sent',error=>qaSignal=String(error));return true}catch(error){return String(error)}})()",
        ), true, "extension notification injection failed");
    });
    let notification_deadline = Instant::now() + Duration::from_secs(8);
    until(
        || {
            host(move |h| {
                assert!(
                    Instant::now() < notification_deadline,
                    "notification was not delivered: host={}, sender={}",
                    evaluate(
                        &h.windows_extensions.installs[&key].bridge.view,
                        "({qaNotification,qaError,qaReports})"
                    ),
                    evaluate(&h.views[&tab], "({qaSignal,id:chrome.runtime.id})")
                );
                evaluate(
                    &h.windows_extensions.installs[&key].bridge.view,
                    "qaReports",
                )
                .as_u64()
                .unwrap_or(0)
                    >= 1
            })
        },
        10,
    );
    for index in 0..20 {
        let window = windows[index % 2];
        host(move |h| {
            h.windows_extensions.installs[&key]
                .bridge
                .view
                .evaluate_script(&format!("window.__zephiumRefresh({window})"))
                .unwrap();
        });
        until(
            || {
                host(move |h| {
                    evaluate(
                        &h.windows_extensions.installs[&key].bridge.view,
                        "qaReports",
                    )
                    .as_u64()
                    .unwrap_or(0)
                        >= index as u64 + 2
                })
            },
            10,
        );
    }
    assert_eq!(
        host(move |h| evaluate(
            &h.windows_extensions.installs[&key].bridge.view,
            "qaSnapshots"
        )),
        2
    );
    println!(
        "RUNTIME_PASS injected broken icon: native fetch/decode failures stopped after two snapshots"
    );
    host(move |h| {
        evaluate(&h.views[&tab], "window.close(); true");
    });
    until(
        || {
            events.lock().unwrap().iter().any(
                |event| matches!(event, EngineEvent::NativeTabCloseRequested {id} if *id == tab),
            )
        },
        10,
    );
    println!("RUNTIME_PASS normal extension tab emitted an authorized native close");
    host(move |h| h.close(tab));
    // Replace the installed package, then construct a tab without any opener.
    let rebuilt_root = root.join("rebuilt-package");
    zephium_webext::archive::copy_dir(&load.root, &rebuilt_root, &Default::default()).unwrap();
    let mut rebuilt = load.clone();
    rebuilt.root = rebuilt_root;
    host(move |h| h.load_windows_extension(profile, rebuilt)).unwrap();
    let restored = ItemId::from(103);
    let target = format!("chrome-extension://{}/popup.html", load.extension_id);
    host(move |h| {
        h.create_view(
            restored,
            Partition::Persistent(profile),
            &target,
            Rect::new(0.0, 0.0, 600.0, 400.0),
            live_token(),
        )
    });
    until(
        || host(move |h| h.views[&restored].navigation.committed_snapshot().is_some()),
        15,
    );
    host(move |h| {
        evaluate(&h.views[&restored], "window.close();true");
    });
    until(
        || {
            events.lock().unwrap().iter().any(|event| matches!(event, EngineEvent::NativeTabCloseRequested {id} if *id == restored))
        },
        10,
    );
    host(move |h| h.close(restored));
    println!(
        "RUNTIME_PASS extension tab close after package replacement and fresh view construction"
    );

    // The production missed-style path over an actually suspended WebView2.
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let url = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().unwrap().port()
    );
    listener.set_nonblocking(true).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let server = std::thread::spawn(move || {
        while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
            if let Ok((mut stream, _)) = listener.accept() {
                let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
                let mut request = [0; 4096];
                let _ = stream.read(&mut request);
                let body = "<!doctype html><div id=ad>Advertisement</div><div id=other>Other</div>";
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
            } else {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    });
    let site = BlockerSite::from_url(&url).unwrap();
    let site_initial = site.clone();
    let web = ItemId::from(102);
    host(move |h| {
        h.set_blocker_site_preferences(
            profile,
            PreparedBlockerSites::new(
                1,
                [(
                    site_initial,
                    false,
                    Arc::from("#ad{display:none!important}"),
                )],
            )
            .unwrap(),
        );
        h.create_view(
            web,
            Partition::Persistent(profile),
            &url,
            Rect::new(0.0, 0.0, 600.0, 400.0),
            live_token(),
        );
    });
    until(
        || {
            host(move |h| {
                evaluate(
                    &h.views[&web],
                    "!!document.querySelector('#ad') && getComputedStyle(document.querySelector('#ad')).display==='none'",
                ) == true
            })
        },
        15,
    );
    host(move |h| {
        h.hidden.insert(web);
        h.views[&web].set_visible(false).unwrap();
        h.set_dormant(vec![web]);
    });
    until(|| host(move |h| h.dormant.contains(&web)), 10);
    host(move |h| {
        h.set_blocker_site_preferences(
            profile,
            PreparedBlockerSites::new(
                2,
                [(site, false, Arc::from("#other{display:none!important}"))],
            )
            .unwrap(),
        );
        assert!(h.styles_missed.contains(&web));
        h.set_dormant(vec![]);
    });
    until(
        || {
            host(move |h| {
                evaluate(
                    &h.views[&web],
                    "getComputedStyle(document.querySelector('#ad')).display !== 'none' && getComputedStyle(document.querySelector('#other')).display === 'none'",
                ) == true
            })
        },
        15,
    );
    assert!(!host(move |h| h.styles_missed.contains(&web)));
    host(move |h| {
        h.close(web);
        h.close(second);
    });
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    server.join().unwrap();
    println!("RUNTIME_PASS sleeping tab applied changed blocker styles after native resume");

    // Actual native popup controller with the production observer and close owner.
    let id = load.extension_id.clone();
    let parent = window.0 .0 as isize;
    let open_popup = move || {
        let id = id.clone();
        host(move |h| {
            let runtime = h.windows_extensions.installs[&key].runtime;
            let window =
                popup::PopupWindow::new(HWND(parent as *mut _), Rect::new(0.0, 0.0, 30.0, 30.0))
                    .unwrap();
            let view = WebViewBuilder::new()
                .with_environment(h.environments[&profile].clone())
                .with_url("about:blank")
                .build_as_child(&window)
                .unwrap();
            let alive = Rc::new(Cell::new(true));
            let observer = h
                .observe_extension_crashes(&view, runtime, alive.clone(), true)
                .unwrap();
            h.windows_extensions.popup = Some(Popup {
                runtime,
                tab,
                window,
                escape: None,
                view: ExtensionView {
                    view,
                    alive,
                    crash_observer: Some(observer),
                    resource: h
                        .native_resources
                        .try_acquire(NativeResourceClass::Extension)
                        .unwrap(),
                },
            });
            h.windows_extensions
                .popup
                .as_ref()
                .unwrap()
                .view
                .view
                .load_url(&format!("chrome-extension://{id}/popup.html"))
                .unwrap();
        });
    };
    open_popup();
    until(
        || {
            host(|h| {
                evaluate(
                    &h.windows_extensions.popup.as_ref().unwrap().view.view,
                    "document.readyState",
                ) == "complete"
            })
        },
        15,
    );
    host(|h| crash(&h.windows_extensions.popup.as_ref().unwrap().view.view));
    until(|| host(|h| h.windows_extensions.popup.is_none()), 10);
    assert!(!host(move |h| h.windows_view_admission_blocked(profile)));
    println!("RUNTIME_PASS popup renderer crash closed its owned controller");
    open_popup();
    host(move |h| {
        let controller = h
            .windows_extensions
            .popup
            .as_ref()
            .unwrap()
            .view
            .view
            .controller();
        wry::fail_next_webview2_controller_close_for_qualification(&controller);
        h.close_windows_extension_popup();
        assert!(h
            .windows_extensions
            .retained_popup_window
            .as_ref()
            .is_some_and(|window| window.strong_count() == 0));
        assert!(!h.windows_view_admission_blocked(profile));
    });
    open_popup();
    host(|h| h.close_windows_extension_popup());
    println!("RUNTIME_PASS one failed native popup close recovered and released parent/admission");

    // Eight managers plus the one reserved popup/removal slot.
    for index in 2..=8 {
        let next = package(&root, index);
        host(move |h| h.load_windows_extension(profile, next)).unwrap();
    }
    let ninth = package(&root, 9);
    let blocked = ninth.clone();
    assert_eq!(
        host(move |h| h.load_windows_extension(profile, blocked)).unwrap_err(),
        zephium_core::extensions::WINDOWS_EXTENSION_CAPACITY_MESSAGE
    );
    // A disabled native package has no manager; removal uses the reserved slot.
    let extra = ninth.clone();
    open_popup();
    host(move |h| {
        let core =
            native::profile(&h.windows_extensions.installs[&key].bridge.view.webview()).unwrap();
        let item = native::add(&core, &extra.root).unwrap();
        native::enable(&item, false).unwrap();
        h.remove_web_extension(profile, extra);
        assert!(!h.windows_view_admission_blocked(profile));
        assert_eq!(h.windows_extensions.installs.len(), 8);
    });
    // Before-native failure must not quarantine even when no temporary slot exists.
    let ninth_failed = ninth.clone();
    host(move |h| {
        let reserved = h
            .native_resources
            .try_acquire(NativeResourceClass::Extension)
            .unwrap();
        h.remove_web_extension(profile, ninth_failed);
        assert!(!h.windows_view_admission_blocked(profile));
        drop(reserved);
    });
    host(move |h| h.unload_web_extension(profile, ExtensionInstallId::from(8)));
    host(move |h| h.load_windows_extension(profile, ninth)).unwrap();
    println!("RUNTIME_PASS cap, disabled removal, pre-native error, and freed-slot admission");
    host(move |h| {
        h.forget_windows_extensions(profile);
        h.begin_content_policy_shutdown();
    });
    assert!(
        shutdown.finish(),
        "native shutdown must prove process-group exit"
    );
    let child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "native_windows_runtime_recovery",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env("ZEPHIUM_QUALIFICATION_RESTART_ROOT", &root)
        .env("ZEPHIUM_QUALIFICATION_RESTART_PROFILE", profile.to_string())
        .status()
        .unwrap();
    assert!(
        child.success(),
        "fresh-process restoration qualification failed"
    );
    println!("RUNTIME_EVIDENCE {}", root.display());
}
