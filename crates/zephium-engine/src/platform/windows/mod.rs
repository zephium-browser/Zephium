     &handler,
        )
    }
    .is_ok()
}

/// Goes to the entry `offset` steps from the current one when it is still at
/// `url`. The list is read again first, so the entry id comes from the
/// browser's list as it is now and not from one shown earlier.
pub fn go_to_history_entry(view: &wry::WebView, offset: i32, url: &str) -> bool {
    let core = view.webview();
    let url = url.to_owned();
    let navigate = core.clone();
    let handler =
        CallDevToolsProtocolMethodCompletedHandler::create(Box::new(move |result, response| {
            let entry = result
                .is_ok()
                .then(|| crate::history_list::parse(&response))
                .flatten()
                .and_then(|history| history.entry_at(offset, &url));
            if let Some(id) = entry {
                let settled =
                    CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(())));
                // SAFETY: exact STA-owned core and immutable argument buffers.
                let _ = unsafe {
                    navigate.CallDevToolsProtocolMethod(
                        &HSTRING::from("Page.navigateToHistoryEntry"),
                        &HSTRING::from(format!(r#"{{"entryId":{id}}}"#)),
                        &settled,
                    )
                };
            }
            Ok(())
        }));
    // SAFETY: exact STA-owned core and immutable argument buffers.
    unsafe {
        core.CallDevToolsProtocolMethod(
            &HSTRING::from("Page.getNavigationHistory"),
            &HSTRING::from("{}"),
            &handler,
        )
    }
    .is_ok()
}

pub fn stop_loading(view: &wry::WebView) {
    let controller = view.controller();
    if let Ok(core) = unsafe { controller.CoreWebView2() } {
        let _ = unsafe { core.Stop() };
    }
}

/// WebView2 has no playback suspension; muting is what it offers, and a
/// page cannot unmute itself.
pub fn set_media_suspended(view: &wry::WebView, suspended: bool) {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;

    if let Ok(core8) = view.webview().cast::<ICoreWebView2_8>() {
        let _ = unsafe { core8.SetIsMuted(suspended) };
    }
}

/// Permission UI belongs to the actual foreground browser window, rather
/// than whichever logical tab was last selected while another app is active.
pub(crate) fn permission_window_is_foreground(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    hwnd != 0 && unsafe { GetForegroundWindow() == HWND(hwnd as *mut std::ffi::c_void) }
}

/// WebView2's native audio bit cannot be overridden by page JavaScript. API
/// or COM failure is uncertainty and therefore a discard veto.
pub fn query_document_activity(view: &wry::WebView, done: impl FnOnce(bool) + 'static) -> bool {
    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2_8;
    use windows_core::Interface;

    let core = view.webview();
    let Ok(core8) = core.cast::<ICoreWebView2_8>() else {
        return false;
    };
    let mut playing = windows_core::BOOL::default();
    if unsafe { core8.IsDocumentPlayingAudio(&mut playing) }.is_err() {
        return false;
    }
    done(!playing.as_bool());
    true
}

mod replay;
pub(crate) use replay::RequestWitness;

pub(crate) fn discard_history_allows(view: &wry::WebView, owned_blank: bool) -> bool {
    let core = view.webview();
    let mut back = windows_core::BOOL::default();
    let mut forward = windows_core::BOOL::default();
    (unsafe { core.CanGoBack(&mut back).is_ok() && core.CanGoForward(&mut forward).is_ok() })
        && !forward.as_bool()
        && (!back.as_bool() || owned_blank)
}

#[cfg(test)]
mod process_exit_tests {
    use super::*;

    fn proof(expected_process_id: u32) -> BrowserProcessExitProof {
        BrowserProcessExitProof {
            expected_process_id,
            generation: BrowserProcessGeneration(7),
            shared: Arc::new(SharedBrowserProcessExitProof {
                data: std::sync::Mutex::new(BrowserProcessExitProofData {
                    state: BrowserProcessExitProofState::Pending,
                    failed: false,
                    exit_action: None,
                }),
                ready: std::sync::Condvar::new(),
            }),
        }
    }

    #[test]
    fn erasure_and_shutdown_share_one_original_live_process_handle() {
        // SAFETY: queries this test's own current process; no foreign process
        // is launched, terminated, or given additional access.
        let id = unsafe { windows::Win32::System::Threading::GetCurrentProcessId() };
        let original = open_browser_process(id).unwrap();
        let erasure = original.clone();
        assert!(Arc::ptr_eq(&original.handle, &erasure.handle));
        let shutdown = BrowserProcessShutdownObligation::new(original.clone(), proof(id)).unwrap();
        assert!(Arc::ptr_eq(&shutdown.process.handle, &erasure.handle));
        assert!(
            BrowserProcessShutdownObligation::new(original.clone(), proof(id.wrapping_add(1)))
                .is_none()
        );
        drop(original);
        assert!(erasure.is_running());
        drop(erasure);
        assert!(shutdown.process.is_running());
        // Shared ownership does not prove exit or turn a pending Environment5
        // registration into a clean zero-obligation shutdown.
        assert_eq!(
            wait_for_browser_process_shutdown(vec![shutdown], std::time::Duration::ZERO),
            (false, false)
        );
    }

    #[test]
    fn queued_group_exit_waits_for_the_held_handle_before_generation_rollover() {
        let proof = proof(41);
        let handle_exited = std::cell::Cell::new(false);
        let pumps = std::cell::Cell::new(0);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        assert!(wait_for_exact_exit_with_pump(
            deadline,
            || match proof.snapshot() {
                BrowserProcessExitProofState::Invalid => Some(false),
                BrowserProcessExitProofState::Exited if handle_exited.get() => Some(true),
                _ => None,
            },
            |_| {
                pumps.set(pumps.get() + 1);
                if pumps.get() == 1 {
                    proof.record(BrowserProcessExitEvent::Exited {
                        expected_process_id: 41,
                        generation: proof.generation(),
                        observed_process_id: 41,
                        failed: false,
                    });
                } else {
                    handle_exited.set(true);
                }
                true
            },
        ));
        assert_eq!(
            pumps.get(),
            2,
            "the group event alone cannot authorize rollover"
        );
        assert!(!browser_process_callback_matches(
            43,
            BrowserProcessGeneration(8),
            41,
            proof.generation(),
        ));
    }

    #[test]
    fn pending_group_exit_cannot_authorize_rollover_after_the_original_deadline() {
        let proof = proof(41);
        let pumped = std::cell::Cell::new(false);
        assert!(!wait_for_exact_exit_with_pump(
            std::time::Instant::now(),
            || (proof.snapshot() == BrowserProcessExitProofState::Exited).then_some(true),
            |_| {
                pumped.set(true);
                true
            },
        ));
        assert!(!pumped.get());
        assert_eq!(proof.snapshot(), BrowserProcessExitProofState::Pending);
    }

    #[test]
    fn failed_browser_exit_allows_cleanup_but_is_not_clean() {
        let proof = proof(42);
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 42,
            generation: proof.generation(),
            observed_process_id: 42,
            failed: true,
        });
        let cleaned = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let done = cleaned.clone();
        assert!(proof.run_after_exit(Box::new(move || {
            done.store(true, std::sync::atomic::Ordering::Release);
        })));
        assert!(cleaned.load(std::sync::atomic::Ordering::Acquire));
        assert!(!proof.exited_cleanly());
    }

    #[test]
    fn only_the_recorded_pid_satisfies_group_exit_proof() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
            failed: false,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Exited
        );
    }

    #[test]
    fn pid_mismatch_permanently_poisons_group_exit_proof() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 42,
            failed: false,
        });
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
            failed: false,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Invalid
        );
    }

    #[test]
    fn unreadable_event_cannot_be_rehabilitated_by_a_later_callback() {
        let proof = proof(41);
        proof.record(BrowserProcessExitEvent::Invalid {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
        });
        proof.record(BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
            failed: false,
        });
        assert_eq!(
            proof.wait_until(std::time::Instant::now()),
            BrowserProcessExitProofState::Invalid
        );
    }

    #[test]
    fn privileged_exit_set_requires_exact_labels_and_distinct_processes() {
        let expected = ["main", "panel"];
        assert!(privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("panel".into(), 11)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("unknown".into(), 11)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &expected,
            &[("main".into(), 10), ("panel".into(), 10)]
        ));
        assert!(!privileged_observer_set_is_exact(
            &["main", "main"],
            &[("main".into(), 10), ("panel".into(), 11)]
        ));
    }

    #[test]
    fn stale_generation_is_rejected_even_when_windows_reuses_the_pid() {
        assert!(!browser_process_callback_matches(
            41,
            BrowserProcessGeneration(8),
            41,
            BrowserProcessGeneration(7),
        ));
        assert!(browser_process_callback_matches(
            41,
            BrowserProcessGeneration(8),
            41,
            BrowserProcessGeneration(8),
        ));
    }

    #[test]
    fn same_pid_reuse_requires_pending_proof_and_unsignalled_exact_handle() {
        assert!(browser_process_reuse_is_safe(41, 41, 41, true, true));
        assert!(!browser_process_reuse_is_safe(41, 41, 41, false, true));
        assert!(!browser_process_reuse_is_safe(41, 41, 41, true, false));
        assert!(!browser_process_reuse_is_safe(41, 42, 41, true, true));
    }

    #[test]
    fn only_a_signalled_exact_handle_proves_browser_process_exit() {
        use windows::Win32::Foundation::WAIT_FAILED;

        assert!(wait_result_proves_exit(WAIT_OBJECT_0));
        assert!(!wait_result_proves_exit(WAIT_TIMEOUT));
        assert!(!wait_result_proves_exit(WAIT_FAILED));
        assert!(!wait_result_proves_exit(WAIT_EVENT(7)));
    }

    #[test]
    fn native_in_private_mode_must_match_both_storage_classes() {
        assert!(in_private_postcondition_holds(true, true));
        assert!(in_private_postcondition_holds(false, false));
        assert!(!in_private_postcondition_holds(true, false));
        assert!(!in_private_postcondition_holds(false, true));
    }

    #[test]
    fn late_group_exit_runs_the_bounded_erasure_continuation_once() {
        let proof = proof(41);
        let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let action_runs = runs.clone();
        assert!(proof.run_after_exit(Box::new(move || {
            action_runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        })));
        assert!(!proof.run_after_exit(Box::new(|| {
            panic!("only one erasure continuation may be registered")
        })));

        let event = BrowserProcessExitEvent::Exited {
            expected_process_id: 41,
            generation: BrowserProcessGeneration(7),
            observed_process_id: 41,
            failed: false,
        };
        proof.record(event);
        proof.record(event);
        assert_eq!(runs.load(std::sync::atomic::Ordering::Relaxed), 1);
    }
}

#[cfg(test)]
mod bootstrap_history_tests {
    use super::blank_bootstrap_only;

    #[test]
    fn only_the_blank_bootstrap_behind_one_page_is_pruned() {
        let history = |current: u32, urls: &[&str]| {
            let entries: Vec<_> = urls
                .iter()
                .map(|url| serde_json::json!({ "url": url }))
                .collect();
            serde_json::json!({ "currentIndex": current, "entries": entries }).to_string()
        };
        assert!(blank_bootstrap_only(&history(
            1,
            &["about:blank", "https://a.test/"]
        )));
        assert!(!blank_bootstrap_only(&history(
            2,
            &["about:blank", "https://a.test/", "https://b.test/"]
        )));
        assert!(!blank_bootstrap_only(&history(
            1,
            &["https://a.test/", "https://b.test/"]
        )));
        assert!(!blank_bootstrap_only("not json"));
    }
}

#[cfg(test)]
mod frame_judge_tests {
    use super::frame_refused;

    #[test]
    fn a_frame_whose_address_cannot_be_read_is_refused() {
        let blocks = |uri: &str| uri.contains("scam");
        assert!(frame_refused(None, blocks));
        assert!(frame_refused(Some("https://scam.example/"), blocks));
        assert!(!frame_refused(Some("https://fine.example/"), blocks));
    }
}

#[cfg(feature = "agentic-browser")]
pub(crate) mod tab_preview;
/// Without the image codec there is nothing to make a preview with.
#[cfg(not(feature = "agentic-browser"))]
pub(crate) mod tab_preview {
    pub(crate) fn capture(_view: &wry::WebView, done: Box<dyn FnOnce(Option<Vec<u8>>) + Send>) {
        done(None);
    }
}
