ate in Work, and for a website only when you allow it."
        ));
        assert!(plist.contains("<key>NSSpeechRecognitionUsageDescription</key>"));
        assert!(plist.contains("Zephium turns what you dictate in Work into text on this Mac."));
        for key in [
            "NSLocationUsageDescription",
            "NSLocationWhenInUseUsageDescription",
        ] {
            assert!(plist.contains(&format!("<key>{key}</key>")), "{key}");
        }
        let entitlements = include_str!("../Entitlements.plist");
        assert!(
            entitlements.contains("<key>com.apple.security.personal-information.location</key>")
        );
    }

    #[test]
    fn macos_bundle_allows_http_only_in_web_content() {
        let plist = include_str!("../Info.plist");
        assert!(plist.contains("<key>NSAppTransportSecurity</key>"));
        assert!(plist.contains("<key>NSAllowsArbitraryLoadsInWebContent</key>\n    <true/>"));
        for forbidden in [
            "NSAllowsArbitraryLoads",
            "NSAllowsArbitraryLoadsForMedia",
            "NSExceptionDomains",
            "NSAllowsLocalNetworking",
        ] {
            assert!(
                !plist.contains(&format!("<key>{forbidden}</key>")),
                "{forbidden}"
            );
        }
    }

    #[test]
    fn macos_bundle_explains_download_folder_consent() {
        let plist = include_str!("../Info.plist");
        for key in [
            "NSDownloadsFolderUsageDescription",
            "NSDesktopFolderUsageDescription",
            "NSDocumentsFolderUsageDescription",
        ] {
            assert!(plist.contains(&format!("<key>{key}</key>")), "{key}");
        }
    }

    #[test]
    fn production_csp_remains_fail_closed_for_privileged_chrome() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let csp = config
            .pointer("/app/security/csp")
            .and_then(serde_json::Value::as_object)
            .expect("production CSP object");

        let directive = |name: &str| {
            csp.get(name)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_else(|| panic!("missing production CSP directive: {name}"))
        };

        assert_eq!(directive("default-src"), "'self'");
        assert_eq!(directive("script-src"), "'self'");
        assert_eq!(
            directive("connect-src"),
            "ipc: http://ipc.localhost",
            "privileged IPC origins must remain exact"
        );
        assert_eq!(directive("style-src"), "'self' 'unsafe-inline'");
        assert_eq!(
            directive("img-src"),
            "'self' data: zephium-media: http://zephium-media.localhost zephium-preview: http://zephium-preview.localhost"
        );
        assert_eq!(directive("font-src"), "'self' data:");
        // Same-origin workers only: the diagram layout engine runs off the page thread.
        assert_eq!(directive("worker-src"), "'self'");

        for name in [
            "child-src",
            "frame-src",
            "media-src",
            "object-src",
            "base-uri",
            "form-action",
            "frame-ancestors",
        ] {
            assert_eq!(directive(name), "'none'", "{name} must remain denied");
        }

        for forbidden in ["'unsafe-eval'", "'unsafe-inline'", "data:", "blob:"] {
            assert!(
                !directive("script-src")
                    .split_ascii_whitespace()
                    .any(|source| source == forbidden),
                "production script-src admitted forbidden source {forbidden}"
            );
        }
    }

    #[test]
    fn linux_initialized_reveal_never_uses_recursive_tao_visibility() {
        let desktop = include_str!("lib.rs");
        let reveal = desktop
            .split("fn show_initialized_main_window")
            .nth(1)
            .and_then(|source| source.split("const NO_AUTHORIZED_EXIT_CODE").next())
            .expect("main-window reveal seam");
        let linux = reveal
            .split("#[cfg(all(unix, not(target_os = \"macos\")))]")
            .nth(1)
            .and_then(|source| {
                source
                    .split("#[cfg(not(all(unix, not(target_os = \"macos\"))))]")
                    .next()
            })
            .expect("Linux reveal branch");
        assert!(linux.contains("platform::imp::show_initialized_top_level(window)"));
        assert!(!linux.contains("window.show()"));

        let adapter = include_str!("platform/linux.rs");
        let top_level = adapter
            .split("fn show_top_level_only")
            .nth(1)
            .and_then(|source| source.split("fn remove_from_actual_parent").next())
            .expect("native top-level-only reveal");
        assert!(top_level.contains("gtk_window.show()"));
        assert!(!top_level.contains("show_all"));
    }

    #[test]
    fn privileged_target_is_resolved_then_checked_before_bootstrap_navigation() {
        #[cfg(not(target_os = "windows"))]
        let base = Url::parse("tauri://localhost").unwrap();
        #[cfg(target_os = "windows")]
        let base = Url::parse("http://tauri.localhost").unwrap();

        let index =
            super::resolve_privileged_target(&base, &tauri::WebviewUrl::App("index.html".into()))
                .unwrap();
        assert_eq!(index.path(), "/browser.html");

        let nested = super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::App("settings/index.html".into()),
        )
        .unwrap();
        assert_eq!(nested.path(), "/settings/index.html");

        assert!(super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::External(Url::parse(super::PRIVILEGED_BOOTSTRAP_URL).unwrap()),
        )
        .is_err());
        assert!(super::resolve_privileged_target(
            &base,
            &tauri::WebviewUrl::External(Url::parse("https://example.com/").unwrap()),
        )
        .is_err());
    }

    #[test]
    fn caller_authorization_is_explicit_and_deny_by_default() {
        use super::CallerPolicy::{Both, Main, Panel};

        assert!(super::caller_allowed(Main, "main"));
        assert!(!super::caller_allowed(Main, "panel"));
        assert!(!super::caller_allowed(Main, "content"));

        assert!(super::caller_allowed(Panel, "panel"));
        assert!(!super::caller_allowed(Panel, "main"));
        assert!(!super::caller_allowed(Panel, "content"));

        assert!(super::caller_allowed(Both, "main"));
        assert!(super::caller_allowed(Both, "panel"));
        assert!(!super::caller_allowed(Both, "content"));
        assert!(!super::caller_allowed(Both, "Main"));
        assert!(!super::caller_allowed(Both, ""));
    }

    #[test]
    fn blocker_diagnostics_are_main_only_and_have_no_profile_selector() {
        let source = include_str!("lib.rs");
        let command = source
            .split("async fn blocker_status(")
            .nth(1)
            .expect("blocker status command")
            .split("#[tauri::command]")
            .next()
            .expect("bounded blocker status command");
        assert!(command.contains("CallerPolicy::Main"));
        assert!(command.contains("focused_content_policy_status()"));
        assert!(command.contains("spawn_blocking"));
        assert!(command.contains("BLOCKER_STATUS_QUERY_TIMEOUT"));
        assert!(command.contains("BLOCKER_STATUS_QUERY_IN_FLIGHT"));
        assert!(!command.contains("ProfileId"));
        assert!(!command.contains("profile:"));

        let delivery = source
            .split("Projection::BlockerStatus(status)")
            .nth(1)
            .expect("blocker projection route")
            .split("Projection::OperationProcessed")
            .next()
            .expect("bounded blocker projection route");
        assert!(delivery.contains("MAIN_LABEL"));
        assert!(delivery.contains("EVENT_BLOCKER_STATUS"));
        assert!(!delivery.contains("PANEL_LABEL"));

        for command_name in [
            "blocker_set_enabled",
            "blocker_retry",
            "blocker_refresh_sources",
        ] {
            let command = source
                .split(&format!("fn {command_name}("))
                .nth(1)
                .expect("blocker mutation command")
                .split("#[tauri::command]")
                .next()
                .expect("bounded blocker mutation command");
            assert!(command.contains("CallerPolicy::Main"));
            assert!(command.contains("dispatch_operation"));
            assert!(!command.contains("ProfileId"));
            assert!(!command.contains("profile:"));
        }
    }

    #[test]
    fn blocker_status_single_flight_flag_has_scope_owned_reset() {
        static FLAG: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        assert!(FLAG
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok());
        {
            let _reset = super::AtomicFlagReset(&FLAG);
            assert!(FLAG.load(std::sync::atomic::Ordering::Acquire));
        }
        assert!(!FLAG.load(std::sync::atomic::Ordering::Acquire));
    }

    #[test]
    fn numeric_inputs_must_be_finite_and_bounded() {
        assert!(super::point_in_bounds(0.0, -1.0));
        assert!(super::point_in_bounds(
            super::MAX_WINDOW_COORDINATE,
            -super::MAX_WINDOW_COORDINATE
        ));
        assert!(!super::point_in_bounds(f64::NAN, 0.0));
        assert!(!super::point_in_bounds(0.0, f64::INFINITY));
        assert!(!super::point_in_bounds(
            super::MAX_WINDOW_COORDINATE + 1.0,
            0.0
        ));

        assert!(super::sidebar_width_in_bounds(super::MIN_SIDEBAR_WIDTH));
        assert!(super::sidebar_width_in_bounds(super::MAX_SIDEBAR_WIDTH));
        assert!(!super::sidebar_width_in_bounds(
            super::MIN_SIDEBAR_WIDTH - 1.0
        ));
        assert!(!super::sidebar_width_in_bounds(f64::NAN));
    }

    #[test]
    fn work_pane_inputs_are_finite_positive_and_trusted() {
        let rect = |x, y, width, height| {
            super::work_pane_rect(zephium_ipc::WorkPaneRect {
                x,
                y,
                width,
                height,
            })
        };
        assert_eq!(
            rect(10.0, 20.0, 640.0, 480.0),
            Some(zephium_core::geometry::Rect::new(10.0, 20.0, 640.0, 480.0))
        );
        assert_eq!(rect(-5.0, 20.0, 640.0, 480.0).map(|r| r.x), Some(-5.0));
        assert!(rect(10.0, 20.0, 0.0, 480.0).is_none());
        assert!(rect(10.0, 20.0, 640.0, -1.0).is_none());
        assert!(rect(f64::NAN, 20.0, 640.0, 480.0).is_none());
        assert!(rect(10.0, 20.0, f64::INFINITY, 480.0).is_none());
        assert!(rect(10.0, 20.0, super::MAX_WINDOW_COORDINATE + 1.0, 480.0).is_none());

        assert!(super::trusted_web_url("https://example.com/path?q=1"));
        assert!(super::trusted_web_url("http://example.com"));
        assert!(!super::trusted_web_url("javascript:alert(1)"));
        assert!(!super::trusted_web_url("https://user:pw@example.com"));
        assert!(!super::trusted_web_url("file:///etc/hosts"));
        assert!(!super::trusted_web_url("https://example.com/\u{7}"));
        assert!(!super::trusted_web_url(&format!(
            "https://example.com/{}",
            "a".repeat(8192)
        )));
    }

    #[test]
    fn text_and_tagged_inputs_are_bounded() {
        assert!(super::bounded("abc", 3));
        assert!(!super::bounded("abcd", 3));
        assert!(super::setting_value_allowed("appearance", "system"));
        assert!(super::setting_value_allowed("appearance", "light"));
        assert!(super::setting_value_allowed("appearance", "dark"));
        assert!(!super::setting_value_allowed("appearance", "sepia"));
        assert!(!super::setting_value_allowed("keymap", "dark"));

        assert!(super::search_action_in_bounds(&SearchAction::ActivateTab {
            id: "a".repeat(super::MAX_ITEM_ID_BYTES),
        }));
        assert!(!super::search_action_in_bounds(
            &SearchAction::ActivateTab {
                id: "a".repeat(super::MAX_ITEM_ID_BYTES + 1),
            }
        ));
        assert!(super::search_action_in_bounds(&SearchAction::OpenUrl {
            url: "a".repeat(super::MAX_NAVIGATION_INPUT_BYTES),
        }));
        assert!(!super::search_action_in_bounds(&SearchAction::OpenUrl {
            url: "a".repeat(super::MAX_NAVIGATION_INPUT_BYTES + 1),
        }));
        assert!(!super::search_action_in_bounds(&SearchAction::RunCommand {
            id: "a".repeat(super::MAX_COMMAND_ID_BYTES + 1),
        }));
    }

    #[test]
    fn appearance_codes_remain_explicit() {
        assert_eq!(super::appearance_code("system"), super::APPEARANCE_SYSTEM);
        assert_eq!(super::appearance_code("light"), super::APPEARANCE_LIGHT);
        assert_eq!(super::appearance_code("dark"), super::APPEARANCE_DARK);
        assert_eq!(super::appearance_code("invalid"), super::APPEARANCE_SYSTEM);
    }

    fn launcher_admitted() -> zephium_ipc::OperationAdmission {
        zephium_ipc::OperationAdmission {
            operation_id: Some("0000000000000001".into()),
            accepted: true,
        }
    }

    #[test]
    fn an_accepted_app_command_settles_the_launcher_at_admission() {
        let (admission, settlement) = super::route_launcher_command(
            |_| super::accepted_ui_operation(),
            || panic!("an app command never reaches the shell"),
        );
        assert!(admission.accepted);
        assert_eq!(settlement, super::LauncherSettlement::Admitted);
    }

    #[test]
    fn an_accepted_shell_command_waits_for_its_disposition() {
        let (admission, settlement) =
            super::route_launcher_command(|to_shell| to_shell(), launcher_admitted);
        assert_eq!(admission.operation_id.as_deref(), Some("0000000000000001"));
        assert_eq!(settlement, super::LauncherSettlement::Shell);
    }

    #[test]
    fn a_refused_app_command_leaves_the_launcher_open() {
        let (admission, settlement) = super::route_launcher_command(
            |_| super::rejected_operation(),
            || panic!("a refused command never reaches the shell"),
        );
        assert!(!admission.accepted);
        assert_eq!(settlement, super::LauncherSettlement::Refused);
    }

    #[test]
    fn a_shell_refusal_is_the_shell_paths_to_settle() {
        // `run_in_shell` already releases the launcher when the shell
        // refuses admission; settling it again here would be a second answer.
        let (admission, settlement) =
            super::route_launcher_command(|to_shell| to_shell(), super::rejected_operation);
        assert!(!admission.accepted);
        assert_eq!(settlement, super::LauncherSettlement::Shell);
    }
}
